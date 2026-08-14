#requires -Version 5.1

[CmdletBinding()]
param(
    [switch]$SelfTest,
    [switch]$SmokeTest,
    [string]$DataRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# WPF must run in an STA runspace. Windows PowerShell 5.1 normally starts in
# STA, but PowerShell 7 does not, so use the Windows-inbox host when needed.
if ([System.Threading.Thread]::CurrentThread.ApartmentState -ne [System.Threading.ApartmentState]::STA) {
    $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $forwardArguments = @('-NoProfile', '-STA', '-File', $PSCommandPath)
    if ($SelfTest) { $forwardArguments += '-SelfTest' }
    if ($SmokeTest) { $forwardArguments += '-SmokeTest' }
    if (-not [string]::IsNullOrWhiteSpace($DataRoot)) {
        $forwardArguments += @('-DataRoot', $DataRoot)
    }
    & $windowsPowerShell @forwardArguments
    exit $LASTEXITCODE
}

Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
Add-Type -AssemblyName System.Xaml
Add-Type -AssemblyName System.Windows.Forms

# Runtime-compiled in-memory helpers keep process I/O and hashing away from the
# WPF thread. Add-Type and all referenced APIs are included with Windows 11.
if (-not ('Graft.Native.ProcessRunner' -as [type])) {
    Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Threading;

namespace Graft.Native
{
    public sealed class ProcessMessage
    {
        public string Stream { get; set; }
        public string Text { get; set; }
    }

    public static class NativeArguments
    {
        internal static string Join(string[] arguments)
        {
            StringBuilder result = new StringBuilder();
            for (int i = 0; i < arguments.Length; i++)
            {
                if (i > 0) result.Append(' ');
                result.Append(Quote(arguments[i] ?? String.Empty));
            }
            return result.ToString();
        }

        public static string Quote(string argument)
        {
            if (argument.Length > 0 && argument.IndexOfAny(new char[] { ' ', '\t', '\n', '\v', '"' }) < 0)
                return argument;

            StringBuilder quoted = new StringBuilder();
            quoted.Append('"');
            int backslashes = 0;
            foreach (char c in argument)
            {
                if (c == '\\')
                {
                    backslashes++;
                    continue;
                }
                if (c == '"')
                {
                    quoted.Append('\\', backslashes * 2 + 1);
                    quoted.Append('"');
                    backslashes = 0;
                    continue;
                }
                if (backslashes > 0)
                {
                    quoted.Append('\\', backslashes);
                    backslashes = 0;
                }
                quoted.Append(c);
            }
            if (backslashes > 0) quoted.Append('\\', backslashes * 2);
            quoted.Append('"');
            return quoted.ToString();
        }
    }

    public sealed class ProcessRunner
    {
        private Process _process;
        private Thread _stdoutThread;
        private Thread _stderrThread;
        private Thread _unicodeLogThread;
        private Thread _waitThread;
        private volatile bool _completed;
        private volatile bool _cancelRequested;
        private volatile bool _processExited;
        private volatile bool _stopReaders;
        private int _exitCode = -1;
        private string _unicodeLogPath;

        public readonly ConcurrentQueue<ProcessMessage> Messages = new ConcurrentQueue<ProcessMessage>();
        public bool IsCompleted { get { return _completed; } }
        public bool CancelRequested { get { return _cancelRequested; } }
        public int ExitCode { get { return _exitCode; } }
        public int ProcessId { get { return _process == null ? -1 : _process.Id; } }

        public static ProcessRunner Start(string executable, string[] arguments)
        {
            ProcessRunner runner = new ProcessRunner();
            runner.StartInternal(executable, arguments);
            return runner;
        }

        private void StartInternal(string executable, string[] arguments)
        {
            string[] effectiveArguments = arguments;
            if (String.Equals(Path.GetFileName(executable), "robocopy.exe", StringComparison.OrdinalIgnoreCase))
            {
                string logDirectory = Path.Combine(Path.GetTempPath(), "Graft");
                Directory.CreateDirectory(logDirectory);
                try
                {
                    foreach (string stalePath in Directory.GetFiles(logDirectory, "robocopy-*.log"))
                    {
                        try
                        {
                            if (File.GetLastWriteTimeUtc(stalePath) >= DateTime.UtcNow.AddDays(-7)) continue;
                            using (FileStream probe = new FileStream(stalePath, FileMode.Open, FileAccess.ReadWrite, FileShare.None)) { }
                            File.Delete(stalePath);
                        }
                        catch { }
                    }
                }
                catch { }
                _unicodeLogPath = Path.Combine(logDirectory, "robocopy-" + Guid.NewGuid().ToString("N") + ".log");
                List<string> augmented = new List<string>(arguments);
                augmented.Add("/UNILOG:" + _unicodeLogPath);
                effectiveArguments = augmented.ToArray();
            }

            ProcessStartInfo startInfo = new ProcessStartInfo();
            startInfo.FileName = executable;
            startInfo.Arguments = NativeArguments.Join(effectiveArguments);
            startInfo.UseShellExecute = false;
            startInfo.CreateNoWindow = true;
            startInfo.RedirectStandardOutput = true;
            startInfo.RedirectStandardError = true;
            Encoding oemEncoding = Encoding.GetEncoding(System.Globalization.CultureInfo.CurrentCulture.TextInfo.OEMCodePage);
            startInfo.StandardOutputEncoding = oemEncoding;
            startInfo.StandardErrorEncoding = oemEncoding;

            _process = new Process();
            _process.StartInfo = startInfo;
            if (!_process.Start()) throw new InvalidOperationException("The process could not be started.");

            _stdoutThread = NewThread(delegate { ReadStream(_process.StandardOutput.BaseStream, "stdout"); }, "Graft stdout");
            _stderrThread = NewThread(delegate { ReadStream(_process.StandardError.BaseStream, "stderr"); }, "Graft stderr");
            if (_unicodeLogPath != null)
                _unicodeLogThread = NewThread(ReadUnicodeLog, "Graft Unicode log");
            _waitThread = NewThread(WaitForCompletion, "Graft process wait");
            _stdoutThread.Start();
            _stderrThread.Start();
            if (_unicodeLogThread != null) _unicodeLogThread.Start();
            _waitThread.Start();
        }

        private static Thread NewThread(ThreadStart action, string name)
        {
            Thread thread = new Thread(action);
            thread.IsBackground = true;
            thread.Name = name;
            return thread;
        }

        private sealed class PrefixStream : Stream
        {
            private readonly byte[] _prefix;
            private int _position;
            private readonly Stream _inner;

            internal PrefixStream(byte[] prefix, Stream inner) { _prefix = prefix; _inner = inner; }
            public override bool CanRead { get { return true; } }
            public override bool CanSeek { get { return false; } }
            public override bool CanWrite { get { return false; } }
            public override long Length { get { throw new NotSupportedException(); } }
            public override long Position { get { throw new NotSupportedException(); } set { throw new NotSupportedException(); } }
            public override void Flush() { }
            public override int Read(byte[] buffer, int offset, int count)
            {
                int copied = 0;
                if (_position < _prefix.Length)
                {
                    copied = Math.Min(count, _prefix.Length - _position);
                    Buffer.BlockCopy(_prefix, _position, buffer, offset, copied);
                    _position += copied;
                    offset += copied;
                    count -= copied;
                }
                if (count > 0)
                {
                    int read = _inner.Read(buffer, offset, count);
                    copied += read;
                }
                return copied;
            }
            public override long Seek(long offset, SeekOrigin origin) { throw new NotSupportedException(); }
            public override void SetLength(long value) { throw new NotSupportedException(); }
            public override void Write(byte[] buffer, int offset, int count) { throw new NotSupportedException(); }
            protected override void Dispose(bool disposing)
            {
                if (disposing) _inner.Dispose();
                base.Dispose(disposing);
            }
        }

        private void ReadStream(Stream stream, string streamName)
        {
            try
            {
                byte[] sample = new byte[256];
                int sampleCount = stream.Read(sample, 0, sample.Length);
                if (sampleCount == 0) return;

                int offset = 0;
                bool unicodeMarker = sampleCount >= 2 && sample[0] == 0xFF && sample[1] == 0xFE;
                if (unicodeMarker) offset = 2;

                Encoding encoding;
                if (unicodeMarker)
                {
                    int zeroBytes = 0;
                    for (int i = offset; i < sampleCount; i++) if (sample[i] == 0) zeroBytes++;
                    encoding = zeroBytes * 4 > (sampleCount - offset)
                        ? Encoding.Unicode
                        : new UTF8Encoding(false, false);
                }
                else
                {
                    encoding = Encoding.GetEncoding(System.Globalization.CultureInfo.CurrentCulture.TextInfo.OEMCodePage);
                }

                byte[] prefix = new byte[sampleCount - offset];
                Buffer.BlockCopy(sample, offset, prefix, 0, prefix.Length);
                using (StreamReader reader = new StreamReader(new PrefixStream(prefix, stream), encoding, false, 4096))
                {
                    string line;
                    while (!_stopReaders && (line = reader.ReadLine()) != null)
                        Messages.Enqueue(new ProcessMessage { Stream = streamName, Text = line });
                }
            }
            catch (Exception ex)
            {
                if (!_cancelRequested && !_stopReaders)
                    Messages.Enqueue(new ProcessMessage { Stream = "stderr", Text = "Output reader error: " + ex.Message });
            }
        }

        private static bool JoinReader(Thread thread, int milliseconds)
        {
            return thread == null || thread.Join(milliseconds);
        }

        private void WaitForCompletion()
        {
            try
            {
                _process.WaitForExit();
                _exitCode = _process.ExitCode;
            }
            catch (Exception ex)
            {
                Messages.Enqueue(new ProcessMessage { Stream = "stderr", Text = "Process wait error: " + ex.Message });
                _exitCode = -1;
            }
            finally
            {
                _processExited = true;
                bool stdoutStopped = JoinReader(_stdoutThread, 5000);
                bool stderrStopped = JoinReader(_stderrThread, 5000);
                bool unicodeStopped = JoinReader(_unicodeLogThread, 5000);
                if (!stdoutStopped || !stderrStopped || !unicodeStopped)
                {
                    _stopReaders = true;
                    try { if (_process != null) _process.StandardOutput.Close(); } catch { }
                    try { if (_process != null) _process.StandardError.Close(); } catch { }
                    stdoutStopped = JoinReader(_stdoutThread, 1000);
                    stderrStopped = JoinReader(_stderrThread, 1000);
                    unicodeStopped = JoinReader(_unicodeLogThread, 1000);
                    if (!stdoutStopped || !stderrStopped || !unicodeStopped)
                        Messages.Enqueue(new ProcessMessage { Stream = "stderr", Text = "One or more output readers did not stop cleanly; the run log may be incomplete." });
                }
                if (_unicodeLogPath != null && unicodeStopped) { try { File.Delete(_unicodeLogPath); } catch { } }
                try { if (_process != null) _process.Dispose(); } catch { }
                _completed = true;
            }
        }

        private void ReadUnicodeLog()
        {
            try
            {
                while (!File.Exists(_unicodeLogPath) && !_processExited) Thread.Sleep(40);
                if (!File.Exists(_unicodeLogPath)) return;

                using (FileStream stream = new FileStream(_unicodeLogPath, FileMode.Open, FileAccess.Read,
                    FileShare.ReadWrite | FileShare.Delete))
                using (StreamReader reader = new StreamReader(stream, Encoding.Unicode, true, 4096))
                {
                    int finalEmptyReads = 0;
                    while (!_stopReaders)
                    {
                        string line = reader.ReadLine();
                        if (line != null)
                        {
                            finalEmptyReads = 0;
                            Messages.Enqueue(new ProcessMessage { Stream = "stdout", Text = line });
                            continue;
                        }
                        if (_processExited)
                        {
                            finalEmptyReads++;
                            if (finalEmptyReads >= 3) break;
                        }
                        Thread.Sleep(40);
                    }
                }
            }
            catch (Exception ex)
            {
                if (!_cancelRequested && !_stopReaders)
                    Messages.Enqueue(new ProcessMessage { Stream = "stderr", Text = "Unicode log reader error: " + ex.Message });
            }
        }

        public void Cancel()
        {
            _cancelRequested = true;
            try
            {
                if (_process != null && !_process.HasExited) _process.Kill();
            }
            catch { }
        }
    }

    public sealed class PathValidationRunner
    {
        private volatile bool _completed;
        private Thread _thread;

        public bool IsCompleted { get { return _completed; } }
        public string Error { get; private set; }

        public static PathValidationRunner Start(string source, string destination, string sourceMode,
            bool dryRun, bool mirror, bool purge)
        {
            PathValidationRunner runner = new PathValidationRunner();
            runner._thread = new Thread(new ThreadStart(delegate
            {
                try { runner.Error = Validate(source, destination, sourceMode, dryRun, mirror, purge); }
                catch (Exception ex) { runner.Error = "Path validation failed: " + ex.Message; }
                finally { runner._completed = true; }
            }));
            runner._thread.IsBackground = true;
            runner._thread.Name = "Graft path validation";
            runner._thread.Start();
            return runner;
        }

        private static string Validate(string source, string destination, string sourceMode,
            bool dryRun, bool mirror, bool purge)
        {
            if (String.IsNullOrWhiteSpace(source)) return "Source path cannot be empty.";
            if (String.IsNullOrWhiteSpace(destination)) return "Destination path cannot be empty.";
            string providerError = ValidateFileSystemSyntax(source);
            if (providerError != null) return providerError;
            providerError = ValidateFileSystemSyntax(destination);
            if (providerError != null) return providerError;
            foreach (char invalid in new char[] { '<', '>', '"', '|', '?', '*' })
                if (source.IndexOf(invalid) >= 0 || destination.IndexOf(invalid) >= 0)
                    return "Path contains invalid character: '" + invalid + "'.";
            if (source.Length > 250 || destination.Length > 250)
                return "Path length exceeds the recommended safe limit of 250 characters.";

            bool sourceFile = File.Exists(source);
            bool sourceDirectory = Directory.Exists(source);
            bool sourceExists = sourceFile || sourceDirectory;
            if (!dryRun && !sourceExists) return "Source path does not exist: " + source;
            if (sourceExists && sourceMode == "Folder" && !sourceDirectory)
                return "Source must be a folder when Source Type is Folder.";
            if (sourceExists && sourceMode == "File" && !sourceFile)
                return "Source must be a file when Source Type is File.";
            if (File.Exists(destination)) return "Destination must be a folder.";

            string sourceFull = Normalize(source);
            string destinationFull = Normalize(destination);
            if (ContainsReparsePoint(sourceFull))
                return "Source paths that pass through a symbolic link or junction are not supported for assured transfers.";
            if (ContainsReparsePoint(destinationFull))
                return "Destination paths that pass through a symbolic link or junction are not supported for assured transfers.";
            if (String.Equals(sourceFull, destinationFull, StringComparison.OrdinalIgnoreCase))
                return "Source and destination cannot be the same path.";

            if (sourceMode == "Folder")
            {
                if (IsChild(destinationFull, sourceFull)) return "Destination cannot be inside the source directory.";
                if ((mirror || purge) && IsChild(sourceFull, destinationFull))
                    return "Source cannot be inside destination when Mirror or Purge is enabled.";
            }
            else
            {
                string target = Normalize(Path.Combine(destinationFull, Path.GetFileName(sourceFull)));
                if (String.Equals(sourceFull, target, StringComparison.OrdinalIgnoreCase))
                    return "The destination resolves to the source file itself.";
            }
            return null;
        }

        private static string ValidateFileSystemSyntax(string path)
        {
            int colon = path.IndexOf(':');
            if (colon >= 0 && !(colon == 1 && Char.IsLetter(path[0])))
                return "Only filesystem paths are supported: " + path;
            return null;
        }

        private static string Normalize(string path)
        {
            string full = Path.GetFullPath(path);
            string root = Path.GetPathRoot(full);
            string trimmed = full.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            string trimmedRoot = root.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            return String.Equals(trimmed, trimmedRoot, StringComparison.OrdinalIgnoreCase) ? root : trimmed;
        }

        private static bool IsChild(string candidate, string parent)
        {
            if (candidate.Length <= parent.Length || !candidate.StartsWith(parent, StringComparison.OrdinalIgnoreCase)) return false;
            if (parent.EndsWith("\\") || parent.EndsWith("/")) return true;
            char separator = candidate[parent.Length];
            return separator == Path.DirectorySeparatorChar || separator == Path.AltDirectorySeparatorChar;
        }

        private static bool ContainsReparsePoint(string path)
        {
            string current = path;
            if (!File.Exists(current) && !Directory.Exists(current)) current = Path.GetDirectoryName(current);
            while (!String.IsNullOrWhiteSpace(current))
            {
                if (File.Exists(current) || Directory.Exists(current))
                {
                    FileAttributes attributes = File.GetAttributes(current);
                    if ((attributes & FileAttributes.ReparsePoint) != 0) return true;
                }
                DirectoryInfo parent = Directory.GetParent(current);
                if (parent == null) break;
                current = parent.FullName;
            }
            return false;
        }
    }

    public sealed class HashRecord
    {
        public string FullPath { get; set; }
        public string RelativePath { get; set; }
        public string Hash { get; set; }
        public long Size { get; set; }
    }

    public sealed class HashMatchRecord
    {
        public string Path { get; set; }
        public string Hash { get; set; }
    }

    public sealed class HashMismatchRecord
    {
        public string Path { get; set; }
        public string SourceHash { get; set; }
        public string DestinationHash { get; set; }
    }

    public sealed class HashComparisonResult
    {
        public HashMatchRecord[] Matched { get; set; }
        public HashMismatchRecord[] Mismatched { get; set; }
        public string[] Missing { get; set; }
        public string[] Extra { get; set; }
    }

    public static class HashComparisonEngine
    {
        public static HashComparisonResult Compare(HashRecord[] source, HashRecord[] destination)
        {
            Dictionary<string, HashRecord> sourceMap = new Dictionary<string, HashRecord>(StringComparer.OrdinalIgnoreCase);
            Dictionary<string, HashRecord> destinationMap = new Dictionary<string, HashRecord>(StringComparer.OrdinalIgnoreCase);
            foreach (HashRecord item in source ?? new HashRecord[0])
                if (item != null && !sourceMap.ContainsKey(item.RelativePath ?? String.Empty)) sourceMap.Add(item.RelativePath ?? String.Empty, item);
            foreach (HashRecord item in destination ?? new HashRecord[0])
                if (item != null && !destinationMap.ContainsKey(item.RelativePath ?? String.Empty)) destinationMap.Add(item.RelativePath ?? String.Empty, item);

            List<HashMatchRecord> matched = new List<HashMatchRecord>();
            List<HashMismatchRecord> mismatched = new List<HashMismatchRecord>();
            List<string> missing = new List<string>();
            List<string> extra = new List<string>();
            foreach (KeyValuePair<string, HashRecord> pair in sourceMap)
            {
                HashRecord destinationRecord;
                if (!destinationMap.TryGetValue(pair.Key, out destinationRecord)) { missing.Add(pair.Key); continue; }
                if (String.Equals(pair.Value.Hash, destinationRecord.Hash, StringComparison.Ordinal))
                    matched.Add(new HashMatchRecord { Path = pair.Key, Hash = pair.Value.Hash });
                else
                    mismatched.Add(new HashMismatchRecord { Path = pair.Key, SourceHash = pair.Value.Hash, DestinationHash = destinationRecord.Hash });
            }
            foreach (KeyValuePair<string, HashRecord> pair in destinationMap)
                if (!sourceMap.ContainsKey(pair.Key)) extra.Add(pair.Key);
            return new HashComparisonResult
            {
                Matched = matched.ToArray(), Mismatched = mismatched.ToArray(),
                Missing = missing.ToArray(), Extra = extra.ToArray()
            };
        }
    }

    public sealed class HashMessage
    {
        public string Kind { get; set; }
        public string Path { get; set; }
        public string Error { get; set; }
        public int Total { get; set; }
        public HashRecord Record { get; set; }
    }

    public sealed class HashRunner
    {
        private readonly object _resultsLock = new object();
        private readonly List<HashRecord> _results = new List<HashRecord>();
        private volatile bool _completed;
        private volatile bool _cancelRequested;
        private Thread _thread;

        public readonly ConcurrentQueue<HashMessage> Messages = new ConcurrentQueue<HashMessage>();
        public bool IsCompleted { get { return _completed; } }
        public bool CancelRequested { get { return _cancelRequested; } }

        public static HashRunner StartDirectory(string root)
        {
            HashRunner runner = new HashRunner();
            runner._thread = NewThread(delegate { runner.RunDirectory(root); });
            runner._thread.Start();
            return runner;
        }

        public static HashRunner StartFile(string path, string displayName)
        {
            HashRunner runner = new HashRunner();
            runner._thread = NewThread(delegate { runner.RunSingleFile(path, displayName); });
            runner._thread.Start();
            return runner;
        }

        private static Thread NewThread(ThreadStart action)
        {
            Thread thread = new Thread(action);
            thread.IsBackground = true;
            thread.Name = "Graft SHA-256";
            return thread;
        }

        public void Cancel() { _cancelRequested = true; }

        public HashRecord[] GetResults()
        {
            lock (_resultsLock) return _results.ToArray();
        }

        private void RunSingleFile(string path, string displayName)
        {
            try
            {
                Messages.Enqueue(new HashMessage { Kind = "Starting", Total = 1 });
                HashOne(path, displayName);
                Messages.Enqueue(new HashMessage { Kind = "Complete" });
            }
            catch (OperationCanceledException)
            {
                Messages.Enqueue(new HashMessage { Kind = "Cancelled", Error = "Cancelled by user" });
            }
            catch (Exception ex)
            {
                Messages.Enqueue(new HashMessage { Kind = "Error", Path = displayName, Error = ex.Message });
            }
            finally { _completed = true; }
        }

        private void RunDirectory(string root)
        {
            try
            {
                string rootFull = Path.GetFullPath(root);
                string pathRoot = Path.GetPathRoot(rootFull);
                string trimmed = rootFull.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
                string trimmedRoot = pathRoot.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
                rootFull = String.Equals(trimmed, trimmedRoot, StringComparison.OrdinalIgnoreCase) ? pathRoot : trimmed;
                List<string> files = CollectFiles(rootFull);
                Messages.Enqueue(new HashMessage { Kind = "Starting", Total = files.Count });
                foreach (string file in files)
                {
                    ThrowIfCancelled();
                    string relative = file.Length > rootFull.Length
                        ? file.Substring(rootFull.Length).TrimStart(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar)
                        : Path.GetFileName(file);
                    HashOne(file, relative);
                }
                Messages.Enqueue(new HashMessage { Kind = "Complete" });
            }
            catch (OperationCanceledException)
            {
                Messages.Enqueue(new HashMessage { Kind = "Cancelled", Error = "Cancelled by user" });
            }
            catch (Exception ex)
            {
                Messages.Enqueue(new HashMessage { Kind = "Error", Error = ex.Message });
            }
            finally { _completed = true; }
        }

        private List<string> CollectFiles(string root)
        {
            if (!Directory.Exists(root)) throw new DirectoryNotFoundException("Directory does not exist: " + root);
            List<string> files = new List<string>();
            Stack<string> directories = new Stack<string>();
            directories.Push(root);
            while (directories.Count > 0)
            {
                ThrowIfCancelled();
                string current = directories.Pop();
                try
                {
                    foreach (string file in Directory.GetFiles(current))
                    {
                        ThrowIfCancelled();
                        try
                        {
                            if ((File.GetAttributes(file) & FileAttributes.ReparsePoint) == 0) files.Add(file);
                        }
                        catch (Exception ex)
                        {
                            Messages.Enqueue(new HashMessage { Kind = "Error", Path = file, Error = ex.Message });
                        }
                    }
                    foreach (string directory in Directory.GetDirectories(current))
                    {
                        ThrowIfCancelled();
                        try
                        {
                            if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) == 0) directories.Push(directory);
                        }
                        catch (Exception ex)
                        {
                            Messages.Enqueue(new HashMessage { Kind = "Error", Path = directory, Error = ex.Message });
                        }
                    }
                }
                catch (Exception ex)
                {
                    Messages.Enqueue(new HashMessage { Kind = "Error", Path = current, Error = ex.Message });
                }
            }
            return files;
        }

        private void HashOne(string path, string relativePath)
        {
            ThrowIfCancelled();
            Messages.Enqueue(new HashMessage { Kind = "FileStarted", Path = relativePath });
            try
            {
                HashRecord record = new HashRecord();
                record.FullPath = path;
                record.RelativePath = relativePath;
                using (FileStream stream = new FileStream(path, FileMode.Open, FileAccess.Read,
                    FileShare.Read, 1024 * 1024, FileOptions.SequentialScan))
                using (SHA256 sha = SHA256.Create())
                {
                    record.Size = stream.Length;
                    byte[] buffer = new byte[64 * 1024];
                    int read;
                    while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
                    {
                        ThrowIfCancelled();
                        sha.TransformBlock(buffer, 0, read, null, 0);
                    }
                    sha.TransformFinalBlock(new byte[0], 0, 0);
                    if (stream.Length != record.Size)
                        throw new IOException("File size changed while hashing: " + path);
                    record.Hash = BitConverter.ToString(sha.Hash).Replace("-", "").ToLowerInvariant();
                }
                lock (_resultsLock) _results.Add(record);
                Messages.Enqueue(new HashMessage { Kind = "FileComplete", Path = relativePath, Record = record });
            }
            catch (OperationCanceledException) { throw; }
            catch (Exception ex)
            {
                Messages.Enqueue(new HashMessage { Kind = "Error", Path = relativePath, Error = ex.Message });
            }
        }

        private void ThrowIfCancelled()
        {
            if (_cancelRequested) throw new OperationCanceledException();
        }
    }
}
'@
}

$script:AppVersion = '1.0.8'
$script:AppTitle = 'GRAFT - Graphical Robocopy Assured File Transfer Tool'

$script:OptionDefinitions = @(
    [pscustomobject]@{ Key = 'copy_subdirs'; Flag = '/S'; Name = 'Copy Subdirectories'; Description = 'Copy subdirectories, excluding empty ones'; Category = 'Copy Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_subdirs_empty'; Flag = '/E'; Name = 'Copy Empty Subdirs'; Description = 'Copy subdirectories, including empty ones'; Category = 'Copy Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_levels'; Flag = '/LEV:'; Name = 'Copy Levels'; Description = 'Only copy the top N levels of the source directory tree'; Category = 'Copy Options'; HasValue = $true; Default = '' },
    [pscustomobject]@{ Key = 'copy_restartable'; Flag = '/Z'; Name = 'Restartable Mode'; Description = 'Copy files in restartable mode (survives network interruptions)'; Category = 'Copy Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_backup'; Flag = '/B'; Name = 'Backup Mode'; Description = 'Copy files in backup mode (requires backup privileges)'; Category = 'Copy Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_unbuffered'; Flag = '/J'; Name = 'Unbuffered I/O'; Description = 'Use unbuffered I/O (recommended for large files)'; Category = 'Copy Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_all'; Flag = '/COPYALL'; Name = 'Copy All Attributes'; Description = 'Copy all file information (DATSOU)'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_flags'; Flag = '/COPY:'; Name = 'Copy Flags'; Description = 'D=Data, A=Attributes, T=Timestamps, S=Security, O=Owner, U=Auditing'; Category = 'File Selection'; HasValue = $true; Default = 'DAT' },
    [pscustomobject]@{ Key = 'dir_copy_flags'; Flag = '/DCOPY:'; Name = 'Dir Copy Flags'; Description = 'D=Data, A=Attributes, T=Timestamps, E=Extended attributes'; Category = 'File Selection'; HasValue = $true; Default = 'DA' },
    [pscustomobject]@{ Key = 'sec_copy'; Flag = '/SEC'; Name = 'Copy Security'; Description = 'Copy files with security (equivalent to /COPY:DATS)'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'copy_timestamps'; Flag = '/TIMFIX'; Name = 'Fix Timestamps'; Description = 'Fix file times on all files, including skipped files'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'purge'; Flag = '/PURGE'; Name = 'Purge Destination'; Description = 'Delete destination items that no longer exist in source'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'mirror'; Flag = '/MIR'; Name = 'Mirror Mode'; Description = 'Mirror a directory tree (/E plus /PURGE)'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'move_files'; Flag = '/MOV'; Name = 'Move Files'; Description = 'Move files and delete them from source after copying'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'move_files_dirs'; Flag = '/MOVE'; Name = 'Move Files and Dirs'; Description = 'Move files and directories and delete them from source'; Category = 'File Selection'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'attr_add'; Flag = '/A+:'; Name = 'Add Attributes'; Description = 'Add attributes R, A, S, H, C, N, E, or T'; Category = 'Attributes'; HasValue = $true; Default = '' },
    [pscustomobject]@{ Key = 'attr_remove'; Flag = '/A-:'; Name = 'Remove Attributes'; Description = 'Remove attributes R, A, S, H, C, N, E, or T'; Category = 'Attributes'; HasValue = $true; Default = '' },
    [pscustomobject]@{ Key = 'create_tree'; Flag = '/CREATE'; Name = 'Create Tree Only'; Description = 'Create the directory tree and zero-length files only'; Category = 'Attributes'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'retry_count'; Flag = '/R:'; Name = 'Retry Count'; Description = 'Number of retries on failed copies'; Category = 'Retry Options'; HasValue = $true; Default = '3' },
    [pscustomobject]@{ Key = 'retry_wait'; Flag = '/W:'; Name = 'Retry Wait'; Description = 'Seconds to wait between retries'; Category = 'Retry Options'; HasValue = $true; Default = '5' },
    [pscustomobject]@{ Key = 'log_verbose'; Flag = '/V'; Name = 'Verbose Output'; Description = 'Show skipped files in output'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'log_timestamps'; Flag = '/TS'; Name = 'Include Timestamps'; Description = 'Include source file timestamps in output'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'log_full_path'; Flag = '/FP'; Name = 'Full Pathnames'; Description = 'Include full file pathnames in output'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'log_bytes'; Flag = '/BYTES'; Name = 'Show Bytes'; Description = 'Print sizes as bytes'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'no_progress'; Flag = '/NP'; Name = 'No Progress'; Description = 'Do not show per-file percentage progress'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'log_eta'; Flag = '/ETA'; Name = 'Show ETA'; Description = 'Show estimated time of arrival for copied files'; Category = 'Logging Options'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'exclude_changed'; Flag = '/XC'; Name = 'Exclude Changed'; Description = 'Exclude changed files'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'exclude_newer'; Flag = '/XN'; Name = 'Exclude Newer'; Description = 'Exclude newer files'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'exclude_older'; Flag = '/XO'; Name = 'Exclude Older'; Description = 'Exclude older files'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'exclude_extra'; Flag = '/XX'; Name = 'Exclude Extra'; Description = 'Exclude extra destination files and directories'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'exclude_lonely'; Flag = '/XL'; Name = 'Exclude Lonely'; Description = 'Exclude source-only files and directories'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'include_same'; Flag = '/IS'; Name = 'Include Same'; Description = 'Overwrite files even when identical'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'include_modified'; Flag = '/IT'; Name = 'Include Tweaked'; Description = 'Include same-size files with different timestamps'; Category = 'File Filters'; HasValue = $false; Default = '' },
    [pscustomobject]@{ Key = 'multi_thread'; Flag = '/MT:'; Name = 'Multi-threaded'; Description = 'Copy with N threads (1 to 128)'; Category = 'Performance'; HasValue = $true; Default = '8' },
    [pscustomobject]@{ Key = 'inter_packet_gap'; Flag = '/IPG:'; Name = 'Inter-Packet Gap'; Description = 'Delay packets by N milliseconds for bandwidth throttling'; Category = 'Performance'; HasValue = $true; Default = '' },
    [pscustomobject]@{ Key = 'dry_run'; Flag = '/L'; Name = 'Dry Run (List Only)'; Description = 'List only; do not copy, delete, or timestamp files'; Category = 'Special'; HasValue = $false; Default = '' }
)

$script:PresetDefinitions = [ordered]@{
    LargeFilesWan = [pscustomobject]@{ Name = 'Large Files over WAN'; Description = 'Optimized for large files over a WAN with unbuffered I/O, conservative threading, and sensible retries.' }
    MirrorWithMetadata = [pscustomobject]@{ Name = 'Mirror with Full Metadata'; Description = 'Creates an exact mirror with data, attributes, timestamps, and security. Extra destination items are deleted.' }
    CopyAllPreserve = [pscustomobject]@{ Name = 'Copy All and Preserve Attributes'; Description = 'Copies all files while preserving data, attributes, timestamps, and security without deleting destination files.' }
    IncrementalBackup = [pscustomobject]@{ Name = 'Incremental Backup'; Description = 'Copies new or changed files and excludes older source files.' }
    QuickCopy = [pscustomobject]@{ Name = 'Quick Copy (No Extras)'; Description = 'A fast copy with minimal options for simple transfers.' }
    None = [pscustomobject]@{ Name = 'Custom'; Description = 'Manually select Robocopy options below.' }
}
$script:PresetOrder = @('LargeFilesWan', 'MirrorWithMetadata', 'CopyAllPreserve', 'IncrementalBackup', 'QuickCopy', 'None')

function Get-GraftProperty {
    param($InputObject, [string]$Name, $Default = $null)
    if ($null -eq $InputObject) { return $Default }
    $property = $InputObject.PSObject.Properties[$Name]
    if ($null -eq $property) { return $Default }
    return $property.Value
}

function Get-GraftOption {
    param($Options, [string]$Key)
    $property = $Options.PSObject.Properties[$Key]
    if ($null -eq $property) { throw "Unknown option: $Key" }
    return $property.Value
}

function New-GraftOptions {
    param([string]$Preset = 'None')
    $properties = [ordered]@{}
    foreach ($definition in $script:OptionDefinitions) {
        $properties[$definition.Key] = [pscustomobject][ordered]@{
            flag = $definition.Flag
            name = $definition.Name
            description = $definition.Description
            enabled = $false
            has_value = [bool]$definition.HasValue
            value = [string]$definition.Default
        }
    }
    $properties.current_preset = 'None'
    $options = [pscustomobject]$properties
    if ($Preset -ne 'None') { Set-GraftPreset -Options $options -Preset $Preset }
    return $options
}

function Reset-GraftOptions {
    param($Options)
    foreach ($definition in $script:OptionDefinitions) {
        $option = Get-GraftOption $Options $definition.Key
        $option.enabled = $false
        $option.value = [string]$definition.Default
    }
    $Options.current_preset = 'None'
}

function Enable-GraftOption {
    param($Options, [string]$Key, [AllowNull()][string]$Value = $null)
    $option = Get-GraftOption $Options $Key
    $option.enabled = $true
    if ($null -ne $Value) { $option.value = $Value }
}

function Set-GraftPreset {
    param($Options, [ValidateSet('LargeFilesWan', 'MirrorWithMetadata', 'CopyAllPreserve', 'IncrementalBackup', 'QuickCopy', 'None')][string]$Preset)
    Reset-GraftOptions $Options
    $Options.current_preset = $Preset
    switch ($Preset) {
        'LargeFilesWan' {
            Enable-GraftOption $Options 'copy_subdirs_empty'
            Enable-GraftOption $Options 'copy_flags' 'DAT'
            Enable-GraftOption $Options 'dir_copy_flags' 'DAT'
            Enable-GraftOption $Options 'copy_unbuffered'
            Enable-GraftOption $Options 'no_progress'
            Enable-GraftOption $Options 'retry_count' '3'
            Enable-GraftOption $Options 'retry_wait' '5'
            Enable-GraftOption $Options 'multi_thread' '8'
        }
        'MirrorWithMetadata' {
            Enable-GraftOption $Options 'mirror'
            Enable-GraftOption $Options 'copy_flags' 'DATS'
            Enable-GraftOption $Options 'copy_restartable'
            Enable-GraftOption $Options 'retry_count' '3'
            Enable-GraftOption $Options 'retry_wait' '5'
            Enable-GraftOption $Options 'multi_thread' '8'
        }
        'CopyAllPreserve' {
            Enable-GraftOption $Options 'copy_subdirs_empty'
            Enable-GraftOption $Options 'copy_flags' 'DATS'
            Enable-GraftOption $Options 'copy_restartable'
            Enable-GraftOption $Options 'retry_count' '3'
            Enable-GraftOption $Options 'retry_wait' '5'
            Enable-GraftOption $Options 'multi_thread' '8'
        }
        'IncrementalBackup' {
            Enable-GraftOption $Options 'copy_subdirs_empty'
            Enable-GraftOption $Options 'copy_flags' 'DAT'
            Enable-GraftOption $Options 'exclude_older'
            Enable-GraftOption $Options 'retry_count' '3'
            Enable-GraftOption $Options 'retry_wait' '5'
            Enable-GraftOption $Options 'multi_thread' '8'
        }
        'QuickCopy' {
            Enable-GraftOption $Options 'copy_subdirs_empty'
            Enable-GraftOption $Options 'multi_thread' '16'
            Enable-GraftOption $Options 'retry_count' '1'
            Enable-GraftOption $Options 'retry_wait' '1'
        }
    }
}

function ConvertTo-GraftOptions {
    param($InputObject)
    $options = New-GraftOptions
    if ($null -eq $InputObject) {
        Set-GraftPreset $options 'LargeFilesWan'
        return $options
    }
    foreach ($definition in $script:OptionDefinitions) {
        $sourceProperty = $InputObject.PSObject.Properties[$definition.Key]
        if ($null -eq $sourceProperty -or $null -eq $sourceProperty.Value) { continue }
        $source = $sourceProperty.Value
        $target = Get-GraftOption $options $definition.Key
        $target.enabled = [bool](Get-GraftProperty $source 'enabled' $false)
        $target.value = [string](Get-GraftProperty $source 'value' $definition.Default)
    }
    $preset = [string](Get-GraftProperty $InputObject 'current_preset' 'None')
    if (-not $script:PresetDefinitions.Contains($preset)) { $preset = 'None' }
    $options.current_preset = $preset
    return $options
}

function Test-GraftOptionsMatchPreset {
    param($Options)
    $preset = [string]$Options.current_preset
    if ($preset -eq 'None') { return $true }
    $expected = New-GraftOptions $preset
    foreach ($definition in $script:OptionDefinitions) {
        $actualOption = Get-GraftOption $Options $definition.Key
        $expectedOption = Get-GraftOption $expected $definition.Key
        if ([bool]$actualOption.enabled -ne [bool]$expectedOption.enabled) { return $false }
        if ($definition.HasValue -and [string]$actualOption.value -cne [string]$expectedOption.value) { return $false }
    }
    return $true
}

function Get-GraftOptionValidationErrors {
    param($Options)
    $errors = New-Object 'System.Collections.Generic.List[string]'
    foreach ($definition in $script:OptionDefinitions | Where-Object HasValue) {
        $option = Get-GraftOption $Options $definition.Key
        if (-not $option.enabled) { continue }
        $value = ([string]$option.value).Trim()
        $error = $null
        if ([string]::IsNullOrWhiteSpace($value)) { $error = 'value cannot be empty' }
        elseif ($value -match '\s') { $error = 'value cannot contain spaces' }
        elseif ($value.Contains('/') -or $value.Contains('\')) { $error = "value cannot contain '/' or '\\'" }
        else {
            switch ($definition.Flag) {
                { $_ -in @('/LEV:', '/R:', '/W:', '/IPG:') } {
                    if ($value -notmatch '^[0-9]+$') { $error = 'value must be a non-negative integer' }
                    break
                }
                '/MT:' {
                    $number = 0
                    if ($value -notmatch '^[0-9]+$' -or -not [int]::TryParse($value, [ref]$number) -or $number -lt 1 -or $number -gt 128) { $error = 'value must be an integer between 1 and 128' }
                    break
                }
                { $_ -in @('/A+:', '/A-:') } {
                    if ($value -notmatch '^[RASHCNET]+$') { $error = 'value must use only attribute letters: R, A, S, H, C, N, E, T' }
                    break
                }
                '/COPY:' {
                    if ($value -notmatch '^[DATSOU]+$') { $error = 'value must use only copy flags: D, A, T, S, O, U' }
                    break
                }
                '/DCOPY:' {
                    if ($value -notmatch '^[DATE]+$') { $error = 'value must use only directory copy flags: D, A, T, E' }
                    break
                }
            }
        }
        if ($null -ne $error) { $errors.Add("$($definition.Name): $error") }
    }
    return $errors.ToArray()
}

function Resolve-GraftTransferSource {
    param([string]$Source, [ValidateSet('Folder', 'File')][string]$Mode)
    if ($Mode -eq 'Folder') { return [pscustomobject]@{ SourceRoot = $Source; FileFilter = $null; SourceLeaf = $null; Mode = 'Folder' } }
    $sourceValue = [System.IO.Path]::GetFullPath($Source)
    $parent = [System.IO.Path]::GetDirectoryName($sourceValue)
    $leaf = [System.IO.Path]::GetFileName($sourceValue)
    if ([string]::IsNullOrWhiteSpace($parent) -or [string]::IsNullOrWhiteSpace($leaf)) { throw 'Source file must have a parent directory and a valid file name.' }
    return [pscustomobject]@{ SourceRoot = $parent; FileFilter = $leaf; SourceLeaf = $leaf; Mode = 'File' }
}

function Get-GraftArguments {
    param($Options, [string]$Source, [string]$Destination, [ValidateSet('Folder', 'File')][string]$SourceMode)
    $resolved = Resolve-GraftTransferSource $Source $SourceMode
    $arguments = New-Object 'System.Collections.Generic.List[string]'
    $arguments.Add([string]$resolved.SourceRoot)
    $arguments.Add($Destination)
    if (-not [string]::IsNullOrWhiteSpace([string]$resolved.FileFilter)) { $arguments.Add([string]$resolved.FileFilter) }
    foreach ($definition in $script:OptionDefinitions) {
        $option = Get-GraftOption $Options $definition.Key
        if (-not $option.enabled) { continue }
        if ($definition.HasValue) {
            $normalizedValue = ([string]$option.value).Trim()
            if (-not [string]::IsNullOrEmpty($normalizedValue)) { $arguments.Add("$($definition.Flag)$normalizedValue") }
        }
        else { $arguments.Add($definition.Flag) }
    }
    # Keep Robocopy traversal aligned with hashing (which does not follow
    # reparse points) and request lossless redirected Unicode output.
    $arguments.Add('/XJ')
    $arguments.Add('/UNICODE')
    return $arguments.ToArray()
}

function ConvertTo-GraftDisplayArgument {
    param([AllowEmptyString()][string]$Argument)
    return [Graft.Native.NativeArguments]::Quote($Argument)
}

function Get-GraftCommandPreview {
    param($Options, [string]$Source, [string]$Destination, [ValidateSet('Folder', 'File')][string]$SourceMode)
    try { $arguments = Get-GraftArguments $Options $Source $Destination $SourceMode }
    catch {
        $arguments = @($Source, $Destination)
        foreach ($definition in $script:OptionDefinitions) {
            $option = Get-GraftOption $Options $definition.Key
            if ($option.enabled) {
                if ($definition.HasValue -and -not [string]::IsNullOrEmpty([string]$option.value)) { $arguments += "$($definition.Flag)$(([string]$option.value).Trim())" }
                elseif (-not $definition.HasValue) { $arguments += $definition.Flag }
            }
        }
        $arguments += '/XJ', '/UNICODE'
    }
    $display = @($arguments | ForEach-Object { ConvertTo-GraftDisplayArgument ([string]$_) })
    return 'robocopy ' + ($display -join ' ')
}

function Get-GraftComparablePath {
    param([string]$Path)
    if (Test-Path -LiteralPath $Path) { $full = (Resolve-Path -LiteralPath $Path).ProviderPath }
    else { $full = [System.IO.Path]::GetFullPath($Path) }
    $pathRoot = [System.IO.Path]::GetPathRoot($full)
    $trimmed = $full.TrimEnd([char]'\', [char]'/')
    $trimmedRoot = $pathRoot.TrimEnd([char]'\', [char]'/')
    if ([string]::Equals($trimmed, $trimmedRoot, [StringComparison]::OrdinalIgnoreCase)) { return $pathRoot }
    return $trimmed
}

function Test-GraftChildPath {
    param([string]$Candidate, [string]$Parent)
    $candidateValue = $Candidate
    $parentValue = $Parent
    if ($candidateValue.Length -le $parentValue.Length) { return $false }
    if (-not $candidateValue.StartsWith($parentValue, [System.StringComparison]::OrdinalIgnoreCase)) { return $false }
    if ($parentValue.EndsWith('\') -or $parentValue.EndsWith('/')) { return $true }
    $separator = $candidateValue[$parentValue.Length]
    return $separator -eq [char]'\' -or $separator -eq [char]'/'
}

function Test-GraftPathContainsReparsePoint {
    param([string]$Path)
    try {
        if (Test-Path -LiteralPath $Path) { $current = (Resolve-Path -LiteralPath $Path).ProviderPath }
        else {
            $full = [System.IO.Path]::GetFullPath($Path)
            $current = [System.IO.Path]::GetDirectoryName($full)
        }
        while (-not [string]::IsNullOrWhiteSpace($current)) {
            if (Test-Path -LiteralPath $current) {
                $item = Get-Item -LiteralPath $current -Force
                if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { return $true }
            }
            $parent = [System.IO.Directory]::GetParent($current)
            if ($null -eq $parent) { break }
            $current = $parent.FullName
        }
    }
    catch { return $false }
    return $false
}

function Test-GraftPaths {
    param([string]$Source, [string]$Destination, [string]$SourceMode, $Options)
    if ([string]::IsNullOrWhiteSpace($Source)) { return 'Source path cannot be empty.' }
    if ([string]::IsNullOrWhiteSpace($Destination)) { return 'Destination path cannot be empty.' }
    foreach ($candidate in @($Source, $Destination)) {
        if (Test-Path -LiteralPath $candidate) {
            try {
                $resolvedProvider = (Resolve-Path -LiteralPath $candidate).Provider.Name
                if ($resolvedProvider -ne 'FileSystem') { return "Only filesystem paths are supported: $candidate" }
            }
            catch { return "Path could not be inspected: $candidate" }
        }
    }
    $dryRun = [bool](Get-GraftOption $Options 'dry_run').enabled
    if (-not $dryRun -and -not (Test-Path -LiteralPath $Source)) { return "Source path does not exist: $Source" }
    if (Test-Path -LiteralPath $Source) {
        if ($SourceMode -eq 'Folder' -and -not (Test-Path -LiteralPath $Source -PathType Container)) { return 'Source must be a folder when Source Type is Folder.' }
        if ($SourceMode -eq 'File' -and -not (Test-Path -LiteralPath $Source -PathType Leaf)) { return 'Source must be a file when Source Type is File.' }
    }
    if ((Test-Path -LiteralPath $Destination) -and -not (Test-Path -LiteralPath $Destination -PathType Container)) { return 'Destination must be a folder.' }
    foreach ($invalid in @('<', '>', '"', '|', '?', '*')) {
        if ($Source.Contains($invalid) -or $Destination.Contains($invalid)) { return "Path contains invalid character: '$invalid'." }
    }
    if ($Source.Length -gt 250 -or $Destination.Length -gt 250) { return 'Path length exceeds the recommended safe limit of 250 characters.' }
    if (Test-GraftPathContainsReparsePoint $Source) { return 'Source paths that pass through a symbolic link or junction are not supported for assured transfers.' }
    if (Test-GraftPathContainsReparsePoint $Destination) { return 'Destination paths that pass through a symbolic link or junction are not supported for assured transfers.' }
    try {
        $sourceComparable = Get-GraftComparablePath $Source
        $destinationComparable = Get-GraftComparablePath $Destination
    }
    catch { return "Path could not be normalized: $($_.Exception.Message)" }
    if ([string]::Equals($sourceComparable, $destinationComparable, [System.StringComparison]::OrdinalIgnoreCase)) { return 'Source and destination cannot be the same path.' }
    if ($SourceMode -eq 'Folder') {
        if (Test-GraftChildPath $destinationComparable $sourceComparable) { return 'Destination cannot be inside the source directory.' }
        $mirror = [bool](Get-GraftOption $Options 'mirror').enabled
        $purge = [bool](Get-GraftOption $Options 'purge').enabled
        if (($mirror -or $purge) -and (Test-GraftChildPath $sourceComparable $destinationComparable)) { return 'Source cannot be inside destination when Mirror or Purge is enabled.' }
    }
    elseif (Test-Path -LiteralPath $Source -PathType Leaf) {
        $target = Join-Path $destinationComparable ([System.IO.Path]::GetFileName($sourceComparable))
        try { $target = Get-GraftComparablePath $target } catch { }
        if ([string]::Equals($sourceComparable, $target, [System.StringComparison]::OrdinalIgnoreCase)) { return 'The destination resolves to the source file itself.' }
    }
    return $null
}

function Disable-GraftFileModeOptions {
    param($Options)
    $disabled = New-Object 'System.Collections.Generic.List[string]'
    foreach ($item in @(
        @('copy_subdirs', 'Copy Subdirectories (/S)'),
        @('copy_subdirs_empty', 'Copy Empty Subdirs (/E)'),
        @('copy_levels', 'Copy Levels (/LEV)'),
        @('mirror', 'Mirror Mode (/MIR)'),
        @('purge', 'Purge Destination (/PURGE)')
    )) {
        $option = Get-GraftOption $Options $item[0]
        if ($option.enabled) { $option.enabled = $false; $disabled.Add($item[1]) }
    }
    if ($disabled.Count -gt 0) { $Options.current_preset = 'None' }
    return $disabled.ToArray()
}

function Get-GraftDestructiveLabels {
    param($Options)
    $labels = New-Object 'System.Collections.Generic.List[string]'
    foreach ($item in @(
        @('mirror', 'Mirror (/MIR)'),
        @('purge', 'Purge (/PURGE)'),
        @('move_files', 'Move files (/MOV)'),
        @('move_files_dirs', 'Move files and directories (/MOVE)')
    )) {
        if ((Get-GraftOption $Options $item[0]).enabled) { $labels.Add($item[1]) }
    }
    return $labels.ToArray()
}

function Test-GraftMoveSourceHashConflict {
    param($Options, [bool]$HashSource)
    if (-not $HashSource) { return $false }
    $moveEnabled = [bool](Get-GraftOption $Options 'move_files').enabled -or [bool](Get-GraftOption $Options 'move_files_dirs').enabled
    $dryRunEnabled = [bool](Get-GraftOption $Options 'dry_run').enabled
    return $moveEnabled -and -not $dryRunEnabled
}

function New-GraftId {
    $milliseconds = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $script:IdSequence = ($script:IdSequence + 1) -band 0xFFFF
    return [int64]($milliseconds * 65536 + $script:IdSequence)
}

function Get-GraftDefaultDataRoot {
    $local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    if ([string]::IsNullOrWhiteSpace($local)) { $local = Join-Path $env:USERPROFILE 'AppData\Local' }
    return Join-Path $local 'Graft'
}

function Write-GraftUtf8File {
    param([string]$Path, [AllowEmptyString()][string]$Content)
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function New-GraftEmptyHistory {
    return [pscustomobject][ordered]@{
        schema_version = 2
        entries = @()
        max_entries = 100
        last_config = $null
        recent_source_paths = @()
        recent_dest_paths = @()
    }
}

function ConvertTo-GraftHistoryEntry {
    param($Entry)
    $timestampValue = Get-GraftProperty $Entry 'timestamp' ([DateTime]::Now.ToString('o'))
    try { $timestamp = ([DateTimeOffset]::Parse([string]$timestampValue)).ToString('o') }
    catch { $timestamp = [DateTime]::Now.ToString('o') }
    $optionsValue = Get-GraftProperty $Entry 'options' $null
    if ($null -eq $optionsValue) {
        $legacyPreset = [string](Get-GraftProperty $Entry 'preset' 'Large Files over WAN')
        $presetKey = 'LargeFilesWan'
        foreach ($key in $script:PresetOrder) {
            if ($script:PresetDefinitions[$key].Name -eq $legacyPreset) { $presetKey = $key; break }
        }
        $options = New-GraftOptions $presetKey
        if ([bool](Get-GraftProperty $Entry 'dryRun' $false)) { (Get-GraftOption $options 'dry_run').enabled = $true; $options.current_preset = 'None' }
    }
    else { $options = ConvertTo-GraftOptions $optionsValue }
    $ticket = Get-GraftProperty $Entry 'ticket_number' (Get-GraftProperty $Entry 'ticketNumber' $null)
    $logContent = Get-GraftProperty $Entry 'log_content' $null
    $logPath = Get-GraftProperty $Entry 'log_path' (Get-GraftProperty $Entry 'logPath' $null)
    $idValue = Get-GraftProperty $Entry 'id' $null
    if ($null -eq $idValue) { $idValue = New-GraftId }
    $sourceValue = [string](Get-GraftProperty $Entry 'source' '')
    $sourceMode = [string](Get-GraftProperty $Entry 'source_mode' (Get-GraftProperty $Entry 'sourceMode' ''))
    if ($sourceMode -notin @('Folder', 'File')) { $sourceMode = 'Folder' }
    return [pscustomobject][ordered]@{
        id = [int64]$idValue
        timestamp = $timestamp
        source = $sourceValue
        source_mode = $sourceMode
        destination = [string](Get-GraftProperty $Entry 'destination' '')
        command = [string](Get-GraftProperty $Entry 'command' '')
        options = $options
        saved = [bool](Get-GraftProperty $Entry 'saved' $false)
        name = Get-GraftProperty $Entry 'name' $null
        log_content = $logContent
        log_path = $logPath
        username = Get-GraftProperty $Entry 'username' $env:USERNAME
        ticket_number = $ticket
        outcome = [string](Get-GraftProperty $Entry 'outcome' '')
    }
}

function Import-GraftHistory {
    $history = New-GraftEmptyHistory
    if (-not (Test-Path -LiteralPath $script:HistoryPath -PathType Leaf)) { return $history }
    try {
        $raw = Get-Content -LiteralPath $script:HistoryPath -Raw
        $loaded = $raw | ConvertFrom-Json
    }
    catch {
        $corruptName = 'history.corrupt.{0}.json' -f (Get-Date -Format 'yyyyMMdd-HHmmss')
        $corruptPath = Join-Path $script:DataRoot $corruptName
        try { [System.IO.File]::Copy($script:HistoryPath, $corruptPath, $true) } catch { }
        $script:StartupWarning = "History could not be read. A recovery copy was saved to $corruptPath"
        return $history
    }
    $entries = New-Object 'System.Collections.Generic.List[object]'
    foreach ($entry in @(Get-GraftProperty $loaded 'entries' @())) {
        try { $entries.Add((ConvertTo-GraftHistoryEntry $entry)) } catch { }
    }
    $history.entries = @($entries.ToArray())
    $history.max_entries = [int](Get-GraftProperty $loaded 'max_entries' (Get-GraftProperty $loaded 'maxEntries' 100))
    if ($history.max_entries -lt 1) { $history.max_entries = 100 }
    $last = Get-GraftProperty $loaded 'last_config' $null
    if ($null -ne $last) {
        $history.last_config = [pscustomobject][ordered]@{
            source = [string](Get-GraftProperty $last 'source' '')
            source_mode = [string](Get-GraftProperty $last 'source_mode' '')
            destination = [string](Get-GraftProperty $last 'destination' '')
            options = ConvertTo-GraftOptions (Get-GraftProperty $last 'options' $null)
        }
    }
    $history.recent_source_paths = @((Get-GraftProperty $loaded 'recent_source_paths' @()) | ForEach-Object { [string]$_ } | Select-Object -First 10)
    $history.recent_dest_paths = @((Get-GraftProperty $loaded 'recent_dest_paths' @()) | ForEach-Object { [string]$_ } | Select-Object -First 10)
    if ($history.recent_source_paths.Count -eq 0) { $history.recent_source_paths = @($history.entries | ForEach-Object source | Where-Object { $_ } | Select-Object -Unique -First 10) }
    if ($history.recent_dest_paths.Count -eq 0) { $history.recent_dest_paths = @($history.entries | ForEach-Object destination | Where-Object { $_ } | Select-Object -Unique -First 10) }
    return $history
}

function Save-GraftHistory {
    if (-not (Test-Path -LiteralPath $script:DataRoot)) { [System.IO.Directory]::CreateDirectory($script:DataRoot) | Out-Null }
    $json = $script:History | ConvertTo-Json -Depth 30
    $tempPath = "$($script:HistoryPath).$PID.tmp"
    Write-GraftUtf8File $tempPath $json
    if (Test-Path -LiteralPath $script:HistoryPath) {
        try { [System.IO.File]::Replace($tempPath, $script:HistoryPath, "$($script:HistoryPath).bak", $true) }
        catch {
            [System.IO.File]::Copy($tempPath, $script:HistoryPath, $true)
            [System.IO.File]::Delete($tempPath)
        }
    }
    else { [System.IO.File]::Move($tempPath, $script:HistoryPath) }
}

function Add-GraftRecentPath {
    param([ValidateSet('Source', 'Destination')][string]$Kind, [string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return }
    $property = if ($Kind -eq 'Source') { 'recent_source_paths' } else { 'recent_dest_paths' }
    $updated = @($Path) + @($script:History.$property | Where-Object { -not [string]::Equals([string]$_, $Path, [StringComparison]::OrdinalIgnoreCase) })
    $script:History.$property = @($updated | Select-Object -First 10)
}

function Add-GraftHistoryEntry {
    param($Entry)
    Add-GraftRecentPath Source $Entry.source
    Add-GraftRecentPath Destination $Entry.destination
    $all = @($Entry) + @($script:History.entries)
    $kept = New-Object 'System.Collections.Generic.List[object]'
    $unsaved = 0
    foreach ($item in $all) {
        if ($item.saved) { $kept.Add($item) }
        else {
            $unsaved++
            if ($unsaved -le $script:History.max_entries) { $kept.Add($item) }
        }
    }
    $script:History.entries = @($kept.ToArray())
}

function Get-GraftHistoryEntryById {
    param($Id)
    $matches = @($script:History.entries | Where-Object { [string]$_.id -eq [string]$Id } | Select-Object -First 1)
    if ($matches.Count -eq 0) { return $null }
    return $matches[0]
}

function Get-GraftHistoryDisplayName {
    param($Entry)
    if (-not [string]::IsNullOrWhiteSpace([string]$Entry.name)) { return [string]$Entry.name }
    $ticket = if ([string]::IsNullOrWhiteSpace([string]$Entry.ticket_number)) { '' } else { " [$($Entry.ticket_number)]" }
    try { $stamp = ([DateTimeOffset]::Parse([string]$Entry.timestamp)).ToLocalTime().ToString('yyyy-MM-dd HH:mm') }
    catch { $stamp = [string]$Entry.timestamp }
    return "$($Entry.source) -> $($Entry.destination)$ticket ($stamp)"
}

function Get-GraftLogFileName {
    param([DateTimeOffset]$Timestamp = [DateTimeOffset]::Now, [AllowNull()][string]$Ticket)
    $base = 'graft_' + $Timestamp.ToLocalTime().ToString('yyyy-MM-dd_HH-mm-ss-fff')
    if (-not [string]::IsNullOrWhiteSpace($Ticket)) {
        $safe = [regex]::Replace($Ticket.Trim(), '[^A-Za-z0-9_-]', '_').Trim('_')
        if ($safe.Length -gt 64) { $safe = $safe.Substring(0, 64) }
        if ([string]::IsNullOrWhiteSpace($safe)) { $safe = 'ticket' }
        $base += '_' + $safe
    }
    return $base + '.log'
}

function Get-GraftUniqueLogPath {
    param([string]$FileName)
    $candidate = Join-Path $script:LogRoot $FileName
    if (-not (Test-Path -LiteralPath $candidate)) { return $candidate }
    $baseName = [System.IO.Path]::GetFileNameWithoutExtension($FileName)
    $extension = [System.IO.Path]::GetExtension($FileName)
    for ($suffix = 1; $suffix -lt 10000; $suffix++) {
        $candidate = Join-Path $script:LogRoot ("$baseName-$suffix$extension")
        if (-not (Test-Path -LiteralPath $candidate)) { return $candidate }
    }
    throw 'A unique automatic log filename could not be allocated.'
}

function New-GraftTransferStats {
    return [pscustomobject][ordered]@{
        files_total = [uint64]0; files_copied = [uint64]0; files_skipped = [uint64]0
        files_mismatch = [uint64]0; files_failed = [uint64]0; files_extras = [uint64]0
        dirs_total = [uint64]0; dirs_copied = [uint64]0; dirs_skipped = [uint64]0
        dirs_failed = [uint64]0; dirs_extras = [uint64]0
        bytes_total = ''; bytes_copied = ''; bytes_failed = ''; speed = ''; robocopy_exit_code = -1
    }
}

function Get-GraftRobocopyExitMessage {
    param([int]$Code)
    switch ($Code) {
        0 { return 'No files were copied. Source and destination are in sync.' }
        1 { return 'All files were copied successfully.' }
        2 { return 'Extra files or directories were detected.' }
        3 { return 'Files were copied and extra items were detected.' }
        4 { return 'Mismatched files or directories were detected.' }
        { $_ -ge 5 -and $_ -le 7 } { return 'Files were copied with some issues.' }
        { $_ -ge 8 -and $_ -le 15 } { return 'Some files or directories could not be copied.' }
        { $_ -ge 16 } { return 'A serious error occurred and no files were copied.' }
        default { return 'The operation ended without a Robocopy exit code.' }
    }
}

function Get-GraftStatNumbers {
    param([string]$Text)
    $values = New-Object 'System.Collections.Generic.List[uint64]'
    foreach ($match in [regex]::Matches($Text, '(?<![\d.])[\d,]+(?![\d.])')) {
        $number = [uint64]0
        if ([uint64]::TryParse($match.Value.Replace(',', ''), [ref]$number)) { $values.Add($number) }
    }
    return $values.ToArray()
}

function Get-GraftByteColumns {
    param([string]$Text)
    $values = New-Object 'System.Collections.Generic.List[string]'
    foreach ($match in [regex]::Matches($Text, '(?i)(?<!\S)([\d,]+(?:\.\d+)?)\s*([kmgt]?)(?=\s|$)')) {
        $value = $match.Groups[1].Value
        if (-not [string]::IsNullOrEmpty($match.Groups[2].Value)) { $value += ' ' + $match.Groups[2].Value }
        $values.Add($value)
    }
    return $values.ToArray()
}

function Update-GraftTransferStats {
    param([int]$ExitCode)
    $stats = New-GraftTransferStats
    $stats.robocopy_exit_code = $ExitCode
    foreach ($line in @($script:ConsoleLines | Select-Object -Last 80)) {
        $text = ([string]$line.Text).Trim()
        if ($text -match '^Dirs\s*:\s*(.*)$') {
            $n = @(Get-GraftStatNumbers $Matches[1])
            if ($n.Count -ge 6) { $stats.dirs_total = $n[0]; $stats.dirs_copied = $n[1]; $stats.dirs_skipped = $n[2]; $stats.dirs_failed = $n[4]; $stats.dirs_extras = $n[5] }
        }
        elseif ($text -match '^Files\s*:\s*(.*)$') {
            $n = @(Get-GraftStatNumbers $Matches[1])
            if ($n.Count -ge 6) { $stats.files_total = $n[0]; $stats.files_copied = $n[1]; $stats.files_skipped = $n[2]; $stats.files_mismatch = $n[3]; $stats.files_failed = $n[4]; $stats.files_extras = $n[5] }
        }
        elseif ($text -match '^Bytes\s*:\s*(.*)$') {
            $columns = @(Get-GraftByteColumns $Matches[1])
            if ($columns.Count -gt 0) { $stats.bytes_total = $columns[0] }
            if ($columns.Count -gt 1) { $stats.bytes_copied = $columns[1] }
            if ($columns.Count -gt 4) { $stats.bytes_failed = $columns[4] }
        }
        elseif ($text -match '^Speed\s*:\s*(.*)$') { $stats.speed = $Matches[1].Trim() }
    }
    $script:TransferStats = $stats
}

function Compare-GraftHashes {
    param([object[]]$SourceHashes, [object[]]$DestinationHashes)
    return [Graft.Native.HashComparisonEngine]::Compare(
        [Graft.Native.HashRecord[]]@($SourceHashes),
        [Graft.Native.HashRecord[]]@($DestinationHashes)
    )
}

$script:IdSequence = 0
$script:StartupWarning = $null
$script:OwnsSelfTestDataRoot = $false
if (-not [string]::IsNullOrWhiteSpace($DataRoot)) { $script:DataRoot = [System.IO.Path]::GetFullPath($DataRoot) }
elseif ($SelfTest) {
    $script:DataRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('GraftSelfTest-' + [guid]::NewGuid().ToString('N'))
    $script:OwnsSelfTestDataRoot = $true
}
else { $script:DataRoot = Get-GraftDefaultDataRoot }
$script:HistoryPath = Join-Path $script:DataRoot 'history.json'
$script:LogRoot = Join-Path $script:DataRoot 'logs'
if (-not (Test-Path -LiteralPath $script:DataRoot)) { [System.IO.Directory]::CreateDirectory($script:DataRoot) | Out-Null }
if (-not (Test-Path -LiteralPath $script:LogRoot)) { [System.IO.Directory]::CreateDirectory($script:LogRoot) | Out-Null }
$script:History = Import-GraftHistory

$script:Options = New-GraftOptions 'LargeFilesWan'
$script:SourcePath = ''
$script:DestinationPath = ''
$script:SourceMode = 'Folder'
if ($null -ne $script:History.last_config) {
    $script:SourcePath = [string]$script:History.last_config.source
    $script:DestinationPath = [string]$script:History.last_config.destination
    $script:Options = ConvertTo-GraftOptions $script:History.last_config.options
    $savedMode = [string](Get-GraftProperty $script:History.last_config 'source_mode' '')
    if ($savedMode -in @('Folder', 'File')) { $script:SourceMode = $savedMode }
    else { $script:SourceMode = 'Folder' }
}

$script:State = 'Idle'
$script:RunContext = $null
$script:ProcessRunner = $null
$script:ValidationRunner = $null
$script:ValidationStartedAt = [DateTime]::MinValue
$script:HashRunner = $null
$script:HashStage = $null
$script:CancelRequested = $false
$script:Outcome = 'Ready'
$script:CloseWhenIdle = $false
$script:CurrentEntryId = $null
$script:SourceHashes = @()
$script:DestinationHashes = @()
$script:SourceHashFailures = @()
$script:DestinationHashFailures = @()
$script:SourceHashFatal = $false
$script:DestinationHashFatal = $false
$script:TransferStats = New-GraftTransferStats
$script:ConsoleLines = New-Object 'System.Collections.Generic.List[object]'
$script:AllConsoleLines = New-Object 'System.Collections.Generic.List[string]'
$script:CapturedFileEntries = New-Object 'System.Collections.Generic.List[object]'
$script:CapturedFileKeys = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
$script:CapturedFileEntriesOmitted = 0
$script:FileStatusMarkers = @(
    @('NEW FILE', 'Copied'), @('NEWER', 'Copied'), @('OLDER', 'Copied'),
    @('CHANGED', 'Copied'), @('TWEAKED', 'Copied'), @('SAME', 'Already Synced')
)
$script:LogEntries = New-Object 'System.Collections.Generic.List[string]'
$script:OptionControls = @{}
$script:UpdatingControls = $false
$script:HistorySelection = $null
$script:Window = $null
$script:Controls = @{}

function Get-GraftConsoleKind {
    param([string]$Text)
    $trimmed = $Text.Trim()
    if ($trimmed.StartsWith('>>>')) { return 'Command' }
    if ($trimmed.Contains('[ERROR]') -or $trimmed.Contains('Error:') -or $trimmed.StartsWith('FAILED:')) { return 'Error' }
    if ($trimmed.Contains('WARNING') -or $trimmed.StartsWith('Warning:')) { return 'Warning' }
    if ($trimmed.Contains('Success') -or $trimmed.Contains('matched perfectly') -or $trimmed.StartsWith('PASSED:')) { return 'Success' }
    if ($trimmed -match '^(Dirs|Files|Bytes|Times|Speed)\s*:') { return 'Summary' }
    return 'Normal'
}

function Capture-GraftFileStatus {
    param([string]$Text)
    $trimmed = $Text.Trim()
    if ([string]::IsNullOrEmpty($trimmed) -or $trimmed.StartsWith('>>>')) { return }
    foreach ($marker in $script:FileStatusMarkers) {
        $index = $trimmed.IndexOf($marker[0], [StringComparison]::OrdinalIgnoreCase)
        if ($index -lt 0) { continue }
        $path = $trimmed.Substring($index + $marker[0].Length).Trim()
        while ($path -match '^([\d,.]+)\s+(.+)$') { $path = $Matches[2].TrimStart() }
        $path = $path.Trim('"').Trim()
        if (-not [string]::IsNullOrWhiteSpace($path)) {
            $key = "$($marker[1])|$path"
            if ($script:CapturedFileKeys.Add($key)) {
                if ($script:CapturedFileEntries.Count -lt 5000) { $script:CapturedFileEntries.Add([pscustomobject]@{ Status = $marker[1]; Path = $path }) }
                else { $script:CapturedFileEntriesOmitted++ }
            }
        }
        return
    }
}

function Get-GraftConsoleBrush {
    param([string]$Kind)
    switch ($Kind) {
        'Command' { return [Windows.Media.Brushes]::DeepSkyBlue }
        'Success' { return [Windows.Media.Brushes]::LightGreen }
        'Warning' { return [Windows.Media.Brushes]::Orange }
        'Error' { return [Windows.Media.Brushes]::Tomato }
        'Summary' { return [Windows.Media.Brushes]::MediumPurple }
        default { return [Windows.Media.Brushes]::Gainsboro }
    }
}

function Write-GraftConsoleRecords {
    param([object[]]$Records, [switch]$Clear)
    if (-not $script:Controls.ContainsKey('ConsoleBox')) { return }
    if ($Clear) { $script:Controls.ConsoleBox.Document.Blocks.Clear() }
    $paragraph = $null
    $inParagraph = 0
    foreach ($line in @($Records)) {
        if ($null -eq $paragraph -or $inParagraph -ge 250) {
            $paragraph = New-Object Windows.Documents.Paragraph
            $paragraph.Margin = New-Object Windows.Thickness(0)
            $paragraph.FontFamily = New-Object Windows.Media.FontFamily('Consolas')
            $paragraph.FontSize = 12
            [void]$script:Controls.ConsoleBox.Document.Blocks.Add($paragraph)
            $inParagraph = 0
        }
        $run = New-Object Windows.Documents.Run(([string]$line.Text) + [Environment]::NewLine)
        $run.Foreground = Get-GraftConsoleBrush $line.Kind
        [void]$paragraph.Inlines.Add($run)
        $inParagraph++
    }
    $script:Controls.ConsoleBox.ScrollToEnd()
}

function Rebuild-GraftConsole {
    if (-not $script:Controls.ContainsKey('ConsoleBox')) { return }
    Write-GraftConsoleRecords @($script:ConsoleLines) -Clear
}

function Add-GraftConsoleBatch {
    param([object[]]$Items)
    $records = New-Object 'System.Collections.Generic.List[object]'
    foreach ($item in @($Items)) {
        $text = [string]$item.Text
        $kind = [string]$item.Kind
        if ([string]::IsNullOrEmpty($kind)) { $kind = Get-GraftConsoleKind $text }
        $record = [pscustomobject]@{ Text = $text; Kind = $kind }
        $script:AllConsoleLines.Add($text)
        Capture-GraftFileStatus $text
        $script:ConsoleLines.Add($record)
        $records.Add($record)
    }
    if ($script:ConsoleLines.Count -gt 2500) {
        $script:ConsoleLines.RemoveRange(0, $script:ConsoleLines.Count - 2000)
        Rebuild-GraftConsole
    }
    elseif ($records.Count -gt 0) { Write-GraftConsoleRecords $records.ToArray() }
}

function Add-GraftConsoleLine {
    param([AllowEmptyString()][string]$Text, [string]$Kind = '')
    Add-GraftConsoleBatch @([pscustomobject]@{ Text = $Text; Kind = $Kind })
}

function Clear-GraftConsole {
    $script:AllConsoleLines.Clear()
    $script:ConsoleLines.Clear()
    $script:CapturedFileEntries.Clear()
    $script:CapturedFileKeys.Clear()
    $script:CapturedFileEntriesOmitted = 0
    if ($script:Controls.ContainsKey('ConsoleBox')) { $script:Controls.ConsoleBox.Document.Blocks.Clear() }
}

function Add-GraftLog {
    param([string]$Message)
    $entry = '[{0}] {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message
    $script:LogEntries.Add($entry)
    if ($script:Controls.ContainsKey('LogBox')) {
        $script:Controls.LogBox.Text = $script:LogEntries -join [Environment]::NewLine
        $script:Controls.LogBox.ScrollToEnd()
    }
}

function Clear-GraftLog {
    $script:LogEntries.Clear()
    if ($script:Controls.ContainsKey('LogBox')) { $script:Controls.LogBox.Clear() }
}

function Set-GraftStatus {
    param([string]$Text, [bool]$Busy = $false, [bool]$Determinate = $false, [double]$Value = 0, [double]$Maximum = 1)
    if (-not $script:Controls.ContainsKey('StatusText')) { return }
    $script:Controls.StatusText.Text = $Text
    $script:Controls.StatusProgress.Visibility = if ($Busy) { 'Visible' } else { 'Collapsed' }
    $script:Controls.StatusProgress.IsIndeterminate = $Busy -and -not $Determinate
    if ($Determinate) {
        $script:Controls.StatusProgress.Minimum = 0
        $script:Controls.StatusProgress.Maximum = [Math]::Max(1, $Maximum)
        $script:Controls.StatusProgress.Value = [Math]::Min($Value, [Math]::Max(1, $Maximum))
    }
}

function Set-GraftBusyUi {
    param([bool]$Busy)
    if (-not $script:Controls.ContainsKey('RunButton')) { return }
    $script:Controls.RunButton.Content = if ($Busy) { 'Cancel' } else { 'Run Robocopy' }
    $script:Controls.ConfigurationPanel.IsEnabled = -not $Busy
    $script:Controls.OptionsTab.IsEnabled = -not $Busy
    $script:Controls.HistoryActionPanel.IsEnabled = -not $Busy
    $script:Controls.ClearUnsavedButton.IsEnabled = -not $Busy
    $script:Controls.ClearConsoleButton.IsEnabled = -not $Busy
    $script:Controls.ClearLogButton.IsEnabled = -not $Busy
    $script:Controls.RunButton.IsEnabled = $true
}

function Sync-GraftOptionsFromControls {
    if ($script:UpdatingControls) { return }
    foreach ($definition in $script:OptionDefinitions) {
        if (-not $script:OptionControls.ContainsKey($definition.Key)) { continue }
        $controls = $script:OptionControls[$definition.Key]
        $option = Get-GraftOption $script:Options $definition.Key
        $option.enabled = [bool]$controls.Check.IsChecked
        if ($definition.HasValue) { $option.value = [string]$controls.Value.Text }
    }
}

function Update-GraftOptionControls {
    $script:UpdatingControls = $true
    try {
        foreach ($definition in $script:OptionDefinitions) {
            if (-not $script:OptionControls.ContainsKey($definition.Key)) { continue }
            $controls = $script:OptionControls[$definition.Key]
            $option = Get-GraftOption $script:Options $definition.Key
            $controls.Check.IsChecked = [bool]$option.enabled
            if ($definition.HasValue) {
                $controls.Value.Text = [string]$option.value
                $controls.Value.IsEnabled = [bool]$option.enabled
            }
        }
        if ($script:Controls.ContainsKey('PresetCombo')) {
            foreach ($item in $script:Controls.PresetCombo.Items) {
                if ([string]$item.Tag -eq [string]$script:Options.current_preset) { $script:Controls.PresetCombo.SelectedItem = $item; break }
            }
            $script:Controls.PresetDescription.Text = $script:PresetDefinitions[[string]$script:Options.current_preset].Description
        }
    }
    finally { $script:UpdatingControls = $false }
    Update-GraftDestructiveBanner
    Update-GraftCommandPreview
}

function Update-GraftDestructiveBanner {
    if (-not $script:Controls.ContainsKey('DestructiveBanner')) { return }
    $labels = @(Get-GraftDestructiveLabels $script:Options)
    if ($labels.Count -gt 0) {
        $script:Controls.DestructiveBanner.Text = 'WARNING: Destructive options enabled - ' + ($labels -join ', ')
        $script:Controls.DestructiveBanner.Visibility = 'Visible'
    }
    else { $script:Controls.DestructiveBanner.Visibility = 'Collapsed' }
}

function Update-GraftPresetAfterManualChange {
    if ($script:UpdatingControls) { return }
    Sync-GraftOptionsFromControls
    if ($script:Options.current_preset -ne 'None' -and -not (Test-GraftOptionsMatchPreset $script:Options)) {
        $script:Options.current_preset = 'None'
        $script:UpdatingControls = $true
        try {
            foreach ($item in $script:Controls.PresetCombo.Items) {
                if ([string]$item.Tag -eq 'None') { $script:Controls.PresetCombo.SelectedItem = $item; break }
            }
            $script:Controls.PresetDescription.Text = $script:PresetDefinitions.None.Description
        }
        finally { $script:UpdatingControls = $false }
    }
    Update-GraftDestructiveBanner
    Update-GraftCommandPreview
}

function Update-GraftCommandPreview {
    if (-not $script:Controls.ContainsKey('CommandPreview')) { return }
    if (-not $script:UpdatingControls) {
        $script:SourcePath = [string]$script:Controls.SourcePathBox.Text
        $script:DestinationPath = [string]$script:Controls.DestinationPathBox.Text
        if ($null -ne $script:Controls.SourceModeCombo.SelectedItem) { $script:SourceMode = [string]$script:Controls.SourceModeCombo.SelectedItem.Content }
    }
    $script:Controls.CommandPreview.Text = Get-GraftCommandPreview $script:Options $script:SourcePath $script:DestinationPath $script:SourceMode
    if ($script:State -eq 'Idle') {
        $script:Controls.RunButton.IsEnabled = -not [string]::IsNullOrWhiteSpace($script:SourcePath) -and -not [string]::IsNullOrWhiteSpace($script:DestinationPath)
    }
}

function Update-GraftRecentCombos {
    if (-not $script:Controls.ContainsKey('RecentSourceCombo')) { return }
    $script:UpdatingControls = $true
    try {
        $script:Controls.RecentSourceCombo.Items.Clear()
        [void]$script:Controls.RecentSourceCombo.Items.Add('Recent...')
        foreach ($path in @($script:History.recent_source_paths)) { [void]$script:Controls.RecentSourceCombo.Items.Add([string]$path) }
        $script:Controls.RecentSourceCombo.SelectedIndex = 0
        $script:Controls.RecentDestinationCombo.Items.Clear()
        [void]$script:Controls.RecentDestinationCombo.Items.Add('Recent...')
        foreach ($path in @($script:History.recent_dest_paths)) { [void]$script:Controls.RecentDestinationCombo.Items.Add([string]$path) }
        $script:Controls.RecentDestinationCombo.SelectedIndex = 0
    }
    finally { $script:UpdatingControls = $false }
}

function Get-GraftStoredSourceMode {
    param([string]$Path)
    foreach ($entry in @($script:History.entries)) {
        if ([string]::Equals([string]$entry.source, $Path, [StringComparison]::OrdinalIgnoreCase) -and ([string]$entry.source_mode -in @('Folder', 'File'))) { return [string]$entry.source_mode }
    }
    return $null
}

function Get-GraftSelectedHistoryEntry {
    if ($null -eq $script:HistorySelection) { return $null }
    return Get-GraftHistoryEntryById $script:HistorySelection
}

function Update-GraftHistoryLists {
    if (-not $script:Controls.ContainsKey('SavedHistoryList')) { return }
    $savedItems = @($script:History.entries | Where-Object saved | ForEach-Object {
        [pscustomobject]@{ Id = [string]$_.id; Name = Get-GraftHistoryDisplayName $_; Command = $_.command; Source = $_.source; Destination = $_.destination; HasLog = $null -ne $_.log_content -or -not [string]::IsNullOrWhiteSpace([string]$_.log_path) }
    })
    $recentItems = @($script:History.entries | Where-Object { -not $_.saved } | ForEach-Object {
        [pscustomobject]@{ Id = [string]$_.id; Name = Get-GraftHistoryDisplayName $_; Command = $_.command; Source = $_.source; Destination = $_.destination; HasLog = $null -ne $_.log_content -or -not [string]::IsNullOrWhiteSpace([string]$_.log_path) }
    })
    $script:Controls.SavedHistoryList.ItemsSource = $savedItems
    $script:Controls.RecentHistoryList.ItemsSource = $recentItems
    $script:HistorySelection = $null
    Update-GraftHistoryButtons
}

function Update-GraftHistoryButtons {
    if (-not $script:Controls.ContainsKey('HistoryLoadButton')) { return }
    $entry = Get-GraftSelectedHistoryEntry
    $enabled = $null -ne $entry -and $script:State -eq 'Idle'
    foreach ($name in @('HistoryLoadButton', 'HistoryRunButton', 'HistorySaveButton', 'HistoryRenameButton', 'HistoryDeleteButton')) { $script:Controls[$name].IsEnabled = $enabled }
    $script:Controls.HistoryExportButton.IsEnabled = $enabled -and ($null -ne $entry.log_content -or -not [string]::IsNullOrWhiteSpace([string]$entry.log_path))
    if ($enabled) { $script:Controls.HistorySaveButton.Content = if ($entry.saved) { 'Unsave' } else { 'Save' } }
    else { $script:Controls.HistorySaveButton.Content = 'Save' }
}

function New-GraftHistoryEntry {
    param($Context)
    return [pscustomobject][ordered]@{
        id = New-GraftId
        timestamp = $Context.StartedAt.ToString('o')
        source = $Context.Source
        source_mode = $Context.SourceMode
        destination = $Context.Destination
        command = $Context.Command
        options = ConvertTo-GraftOptions $Context.Options
        saved = $false
        name = $null
        log_content = $null
        log_path = $null
        username = $env:USERNAME
        ticket_number = if ([string]::IsNullOrWhiteSpace($Context.Ticket)) { $null } else { $Context.Ticket }
        outcome = 'Running'
    }
}

function Invoke-GraftRequestRun {
    if ($script:State -ne 'Idle') { Invoke-GraftCancel; return }
    Sync-GraftOptionsFromControls
    $script:SourcePath = [string]$script:Controls.SourcePathBox.Text
    $script:DestinationPath = [string]$script:Controls.DestinationPathBox.Text
    $script:SourceMode = [string]$script:Controls.SourceModeCombo.SelectedItem.Content

    if ($script:SourceMode -eq 'File') {
        $disabled = @(Disable-GraftFileModeOptions $script:Options)
        if ($disabled.Count -gt 0) {
            Update-GraftOptionControls
            Add-GraftConsoleLine 'Warning: File source mode disabled incompatible directory options:' 'Warning'
            foreach ($label in $disabled) { Add-GraftConsoleLine "  $label" 'Warning' }
        }
    }

    $optionErrors = @(Get-GraftOptionValidationErrors $script:Options)
    if ($optionErrors.Count -gt 0) {
        $message = "Invalid custom option values:`n`n" + ($optionErrors -join "`n")
        Add-GraftLog 'Validation Error: invalid custom option values'
        Add-GraftConsoleLine 'Error: Invalid custom option values:' 'Error'
        foreach ($error in $optionErrors) { Add-GraftConsoleLine "  $error" 'Error' }
        [void][Windows.MessageBox]::Show($script:Window, $message, 'GRAFT Validation', 'OK', 'Error')
        return
    }

    if (Test-GraftMoveSourceHashConflict $script:Options ([bool]$script:Controls.HashSourceCheck.IsChecked)) {
        $message = 'Source hashing runs after Robocopy, but /MOV and /MOVE delete the source after copying. Disable Source File Hash before running a move. Destination hashing may remain enabled.'
        Add-GraftConsoleLine "Error: $message" 'Error'
        [void][Windows.MessageBox]::Show($script:Window, $message, 'Move and Source Hashing Are Incompatible', 'OK', 'Warning')
        return
    }

    $dryRun = [bool](Get-GraftOption $script:Options 'dry_run').enabled
    $mirror = [bool](Get-GraftOption $script:Options 'mirror').enabled
    $purge = [bool](Get-GraftOption $script:Options 'purge').enabled
    try {
        $script:ValidationRunner = [Graft.Native.PathValidationRunner]::Start(
            $script:SourcePath, $script:DestinationPath, $script:SourceMode, $dryRun, $mirror, $purge)
        $script:ValidationStartedAt = [DateTime]::UtcNow
        $script:State = 'Validating'
        $script:Outcome = 'Validating'
        Set-GraftBusyUi $true
        $script:Controls.StatusText.Foreground = [Windows.Media.Brushes]::Gainsboro
        Set-GraftStatus 'Checking source and destination...' $true
    }
    catch {
        Complete-GraftPathValidation "Path validation could not start: $($_.Exception.Message)"
    }
}

function Complete-GraftPathValidation {
    param([AllowNull()][string]$ErrorMessage)
    $script:ValidationRunner = $null
    if (-not [string]::IsNullOrWhiteSpace($ErrorMessage)) {
        $script:State = 'Idle'
        $script:Outcome = 'ValidationFailed'
        Set-GraftBusyUi $false
        Set-GraftStatus 'Path validation failed'
        $script:Controls.StatusText.Foreground = [Windows.Media.Brushes]::Tomato
        Add-GraftLog "Validation Error: $ErrorMessage"
        Add-GraftConsoleLine "Error: $ErrorMessage" 'Error'
        if (-not $SelfTest -and -not $SmokeTest) { [void][Windows.MessageBox]::Show($script:Window, $ErrorMessage, 'GRAFT Validation', 'OK', 'Error') }
        return
    }

    $destructive = @(Get-GraftDestructiveLabels $script:Options)
    if ($destructive.Count -gt 0) {
        $message = "Destructive options are enabled. These options can delete or move files.`n`nEnabled:`n- " + ($destructive -join "`n- ") + "`n`nReview the source and destination paths before continuing."
        $answer = [Windows.MessageBox]::Show($script:Window, $message, 'Destructive Options Enabled', 'YesNo', 'Warning')
        if ($answer -ne [Windows.MessageBoxResult]::Yes) {
            $script:State = 'Idle'
            $script:Outcome = 'Ready'
            Set-GraftBusyUi $false
            Set-GraftStatus 'Ready'
            return
        }
    }
    Start-GraftTransfer
}

function Receive-GraftPathValidation {
    if ($null -eq $script:ValidationRunner) { return }
    if (-not $script:ValidationRunner.IsCompleted) {
        if ([DateTime]::UtcNow -ge $script:ValidationStartedAt.AddSeconds(20)) {
            Complete-GraftPathValidation 'Path validation timed out after 20 seconds. Check whether a network path, mapped drive, or removable drive is unavailable.'
        }
        return
    }
    $validationError = [string]$script:ValidationRunner.Error
    Complete-GraftPathValidation $validationError
}

function Start-GraftTransfer {
    $optionsSnapshot = ConvertTo-GraftOptions $script:Options
    $arguments = @(Get-GraftArguments $optionsSnapshot $script:SourcePath $script:DestinationPath $script:SourceMode)
    $command = Get-GraftCommandPreview $optionsSnapshot $script:SourcePath $script:DestinationPath $script:SourceMode
    $script:RunContext = [pscustomobject][ordered]@{
        Source = $script:SourcePath
        Destination = $script:DestinationPath
        SourceMode = $script:SourceMode
        Ticket = [string]$script:Controls.TicketBox.Text
        Options = $optionsSnapshot
        Arguments = [string[]]$arguments
        Command = $command
        HashSource = [bool]$script:Controls.HashSourceCheck.IsChecked
        HashDestination = [bool]$script:Controls.HashDestinationCheck.IsChecked
        StartedAt = [DateTimeOffset]::Now
    }

    Clear-GraftLog
    Clear-GraftConsole
    $script:CancelRequested = $false
    $script:Outcome = 'Running'
    $script:SourceHashes = @()
    $script:DestinationHashes = @()
    $script:SourceHashFailures = @()
    $script:DestinationHashFailures = @()
    $script:SourceHashFatal = $false
    $script:DestinationHashFatal = $false
    $script:TransferStats = New-GraftTransferStats
    Add-GraftLog "Preset: $($script:PresetDefinitions[[string]$optionsSnapshot.current_preset].Name)"
    Add-GraftLog "Starting: $command"
    Add-GraftConsoleLine ">>> $command" 'Command'
    Add-GraftConsoleLine ''

    $entry = New-GraftHistoryEntry $script:RunContext
    $script:CurrentEntryId = $entry.id
    Add-GraftHistoryEntry $entry
    $script:History.last_config = [pscustomobject][ordered]@{
        source = $script:RunContext.Source
        source_mode = $script:RunContext.SourceMode
        destination = $script:RunContext.Destination
        options = ConvertTo-GraftOptions $script:RunContext.Options
    }
    try { Save-GraftHistory } catch { Add-GraftConsoleLine "Warning: History could not be saved: $($_.Exception.Message)" 'Warning' }
    Update-GraftRecentCombos
    Update-GraftHistoryLists

    $robocopyPath = Join-Path $env:SystemRoot 'System32\robocopy.exe'
    if (-not (Test-Path -LiteralPath $robocopyPath -PathType Leaf)) { $robocopyPath = 'robocopy.exe' }
    try {
        $script:ProcessRunner = [Graft.Native.ProcessRunner]::Start($robocopyPath, [string[]]$script:RunContext.Arguments)
        $script:State = 'Running'
        Set-GraftBusyUi $true
        $script:Controls.StatusText.Foreground = [Windows.Media.Brushes]::Gainsboro
        Set-GraftStatus 'Running Robocopy...' $true
    }
    catch {
        Add-GraftLog "Failed to start Robocopy: $($_.Exception.Message)"
        Add-GraftConsoleLine "Error: Failed to start Robocopy: $($_.Exception.Message)" 'Error'
        $script:TransferStats.robocopy_exit_code = -1
        $script:Outcome = 'CopyFailed'
        Complete-GraftOperation
    }
}

function Invoke-GraftCancel {
    if ($script:State -eq 'Idle' -or $script:State -eq 'Cancelling') { return }
    if ($script:State -eq 'Validating') {
        $script:ValidationRunner = $null
        $script:CancelRequested = $true
        $script:Outcome = 'Cancelled'
        $script:State = 'Idle'
        Add-GraftConsoleLine 'Warning: Path validation cancelled.' 'Warning'
        Add-GraftLog 'Path validation cancelled by user.'
        Set-GraftBusyUi $false
        Set-GraftStatus 'Cancelled'
        return
    }
    $script:CancelRequested = $true
    $script:Outcome = 'Cancelled'
    $script:State = 'Cancelling'
    Add-GraftLog 'Cancellation requested by user.'
    Add-GraftConsoleLine 'Warning: Operation cancellation requested by user.' 'Warning'
    Set-GraftStatus 'Cancelling...' $true
    if ($null -ne $script:ProcessRunner) { $script:ProcessRunner.Cancel() }
    if ($null -ne $script:HashRunner) { $script:HashRunner.Cancel() }
}

function Receive-GraftProcessOutput {
    if ($null -eq $script:ProcessRunner) { return }
    $message = $null
    $drained = 0
    $budget = [Diagnostics.Stopwatch]::StartNew()
    $batch = New-Object 'System.Collections.Generic.List[object]'
    while ($drained -lt 100 -and $budget.ElapsedMilliseconds -lt 12 -and $script:ProcessRunner.Messages.TryDequeue([ref]$message)) {
        if ($message.Stream -eq 'stderr') { $batch.Add([pscustomobject]@{ Text = "[ERROR] $($message.Text)"; Kind = 'Error' }) }
        else { $batch.Add([pscustomobject]@{ Text = [string]$message.Text; Kind = '' }) }
        $drained++
        $message = $null
    }
    if ($batch.Count -gt 0) { Add-GraftConsoleBatch $batch.ToArray() }
    if ($script:ProcessRunner.IsCompleted) {
        if ($script:ProcessRunner.Messages.IsEmpty) { Complete-GraftRobocopy }
        elseif ($script:State -ne 'Cancelling') { Set-GraftStatus 'Processing remaining Robocopy output...' $true }
    }
}

function Complete-GraftInternalFailure {
    param([string]$Stage, $Failure)
    if ($script:State -eq 'Idle') { return }
    $detail = if ($null -eq $Failure) { 'Unknown internal error.' } else { [string]$Failure.Exception.Message }
    $script:CancelRequested = $true
    $script:Outcome = 'InternalError'
    try { if ($null -ne $script:ProcessRunner) { $script:ProcessRunner.Cancel() } } catch { }
    try { if ($null -ne $script:HashRunner) { $script:HashRunner.Cancel() } } catch { }
    $script:ProcessRunner = $null
    $script:HashRunner = $null
    $script:ValidationRunner = $null
    try { Add-GraftConsoleLine "Error: $Stage failed internally: $detail" 'Error' } catch { }
    try { Add-GraftLog "$Stage failed internally: $detail" } catch { }
    Complete-GraftOperation
}

function Complete-GraftRobocopy {
    try {
        $runner = $script:ProcessRunner
        $script:ProcessRunner = $null
        $exitCode = if ($null -eq $runner) { -1 } else { [int]$runner.ExitCode }
        Update-GraftTransferStats $exitCode
        Add-GraftConsoleLine ''
        if ($script:CancelRequested) {
            $script:Outcome = 'Cancelled'
            Add-GraftConsoleLine '>>> Operation cancelled.' 'Warning'
            Add-GraftLog "Robocopy cancelled (process exit code $exitCode)."
            Complete-GraftOperation
            return
        }
        Add-GraftConsoleLine ">>> Robocopy finished with exit code: $exitCode" 'Command'
        $message = Get-GraftRobocopyExitMessage $exitCode
        Add-GraftConsoleLine ">>> $message" $(if ($exitCode -ge 8) { 'Error' } else { 'Command' })
        Add-GraftLog "Robocopy completed with exit code: $exitCode"
        Add-GraftLog $message
        Add-GraftLog ("Files - Total: {0}, Copied: {1}, Skipped: {2}, Failed: {3}, Extras: {4}" -f $script:TransferStats.files_total, $script:TransferStats.files_copied, $script:TransferStats.files_skipped, $script:TransferStats.files_failed, $script:TransferStats.files_extras)
        Add-GraftLog ("Dirs  - Total: {0}, Copied: {1}, Skipped: {2}, Failed: {3}, Extras: {4}" -f $script:TransferStats.dirs_total, $script:TransferStats.dirs_copied, $script:TransferStats.dirs_skipped, $script:TransferStats.dirs_failed, $script:TransferStats.dirs_extras)
        if ($exitCode -lt 0 -or $exitCode -ge 8) { $script:Outcome = 'CopyFailed' }
        if (($script:RunContext.HashSource -or $script:RunContext.HashDestination) -and $exitCode -ge 0 -and $exitCode -lt 8) {
            if ($script:RunContext.HashSource) { Start-GraftHashStage 'Source' }
            else { Start-GraftHashStage 'Destination' }
        }
        elseif ($script:RunContext.HashSource -or $script:RunContext.HashDestination) {
            Add-GraftLog 'Skipping hash operations because Robocopy reported errors.'
            Add-GraftConsoleLine '>>> Skipping hash operations because Robocopy reported errors.' 'Warning'
            Complete-GraftOperation
        }
        else { $script:Outcome = 'Completed'; Complete-GraftOperation }
    }
    catch { Complete-GraftInternalFailure 'Robocopy completion' $_ }
}

function Start-GraftHashStage {
    param([ValidateSet('Source', 'Destination')][string]$Stage)
    $script:HashStage = $Stage
    $script:State = 'Hashing'
    $script:Outcome = 'Hashing'
    $label = $Stage.ToLowerInvariant()
    Add-GraftConsoleLine ''
    Add-GraftConsoleLine ">>> Starting $label file hashing..." 'Command'
    Add-GraftLog "Starting $label file hashing..."
    Set-GraftStatus "Initializing $label hashing..." $true
    try {
        if ($script:RunContext.SourceMode -eq 'File') {
            $leaf = [System.IO.Path]::GetFileName($script:RunContext.Source)
            $path = if ($Stage -eq 'Source') { $script:RunContext.Source } else { Join-Path $script:RunContext.Destination $leaf }
            $script:HashRunner = [Graft.Native.HashRunner]::StartFile($path, $leaf)
        }
        else {
            $path = if ($Stage -eq 'Source') { $script:RunContext.Source } else { $script:RunContext.Destination }
            $script:HashRunner = [Graft.Native.HashRunner]::StartDirectory($path)
        }
    }
    catch {
        Add-GraftConsoleLine ">>> $Stage hashing failed: $($_.Exception.Message)" 'Error'
        if ($Stage -eq 'Source') { $script:SourceHashFatal = $true } else { $script:DestinationHashFatal = $true }
        Complete-GraftHashStage
    }
}

function Receive-GraftHashOutput {
    if ($null -eq $script:HashRunner) { return }
    $message = $null
    $drained = 0
    $budget = [Diagnostics.Stopwatch]::StartNew()
    $lastProgressPath = $null
    $progressChanged = $false
    while ($drained -lt 500 -and $budget.ElapsedMilliseconds -lt 12 -and $script:HashRunner.Messages.TryDequeue([ref]$message)) {
        switch ([string]$message.Kind) {
            'Starting' {
                $script:HashFilesTotal = [int]$message.Total
                $script:HashFilesProcessed = 0
                $progressChanged = $true
            }
            'FileStarted' { $lastProgressPath = [string]$message.Path; $progressChanged = $true }
            'FileComplete' {
                $script:HashFilesProcessed++
                $lastProgressPath = [string]$message.Record.RelativePath
                $progressChanged = $true
            }
            'Error' {
                if ([string]::IsNullOrWhiteSpace([string]$message.Path)) {
                    if ($script:HashStage -eq 'Source') { $script:SourceHashFatal = $true } else { $script:DestinationHashFatal = $true }
                    Add-GraftConsoleLine ">>> Hash error: $($message.Error)" 'Error'
                    Add-GraftLog "Hash error: $($message.Error)"
                }
                else {
                    if ($script:HashStage -eq 'Source') { if ($script:SourceHashFailures -notcontains [string]$message.Path) { $script:SourceHashFailures += [string]$message.Path } }
                    else { if ($script:DestinationHashFailures -notcontains [string]$message.Path) { $script:DestinationHashFailures += [string]$message.Path } }
                    Add-GraftConsoleLine ">>> Hash error [$($message.Path)]: $($message.Error)" 'Error'
                    Add-GraftLog "Hash error [$($message.Path)]: $($message.Error)"
                }
            }
            'Cancelled' { Add-GraftConsoleLine '>>> Hashing cancelled by user.' 'Warning' }
        }
        $drained++
        $message = $null
    }
    if ($progressChanged) {
        $statusText = if ([string]::IsNullOrWhiteSpace($lastProgressPath)) { "Hashing $($script:HashFilesTotal) $($script:HashStage.ToLowerInvariant()) files..." } else { "Hashing: $lastProgressPath" }
        Set-GraftStatus $statusText $true $true $script:HashFilesProcessed $script:HashFilesTotal
    }
    if ($script:HashRunner.IsCompleted) {
        if ($script:HashRunner.Messages.IsEmpty) { Complete-GraftHashStage }
        elseif ($script:State -ne 'Cancelling') { Set-GraftStatus 'Processing remaining hash results...' $true }
    }
}

function Show-GraftHashSummary {
    param([string]$Stage, [object[]]$Hashes, [string[]]$Failures, [bool]$Fatal)
    $label = $Stage.ToLowerInvariant()
    Add-GraftConsoleLine ''
    Add-GraftConsoleLine ">>> $Stage File Hash Summary:" 'Command'
    Add-GraftConsoleLine "  Total hashed files: $($Hashes.Count)"
    $previewCount = [Math]::Min(25, $Hashes.Count)
    if ($previewCount -gt 0) { Add-GraftConsoleLine "  Showing first $previewCount files:" }
    foreach ($hash in @($Hashes | Select-Object -First 25)) { Add-GraftConsoleLine ("  {0} | SHA-256: {1} | {2} bytes" -f $hash.RelativePath, $hash.Hash, $hash.Size) }
    if ($Hashes.Count -gt 25) { Add-GraftConsoleLine "  ... $($Hashes.Count - 25) additional files omitted from live console output" }
    Add-GraftLog "$Stage file hashing complete: $($Hashes.Count) files hashed"
    if ($Failures.Count -gt 0 -or $Fatal) {
        Add-GraftConsoleLine ">>> $Stage hashing warning: $($Failures.Count) paths could not be hashed" 'Warning'
        foreach ($path in @($Failures | Select-Object -First 25)) { Add-GraftConsoleLine "  Warning: $path" 'Warning' }
        if ($Failures.Count -gt 25) { Add-GraftConsoleLine "  ... $($Failures.Count - 25) additional hash failures omitted" 'Warning' }
        Add-GraftLog "$Stage hashing completed with errors."
    }
}

function Complete-GraftHashStage {
    try {
        $runner = $script:HashRunner
        $script:HashRunner = $null
        $results = @()
        if ($null -ne $runner) { $results = @($runner.GetResults()) }
        if ($script:HashStage -eq 'Source') {
            $script:SourceHashes = @($results)
            Show-GraftHashSummary 'Source' $script:SourceHashes $script:SourceHashFailures $script:SourceHashFatal
        }
        else {
            $script:DestinationHashes = @($results)
            Show-GraftHashSummary 'Destination' $script:DestinationHashes $script:DestinationHashFailures $script:DestinationHashFatal
        }
        if ($script:CancelRequested) { $script:Outcome = 'Cancelled'; Complete-GraftOperation; return }
        if ($script:HashStage -eq 'Source' -and $script:RunContext.HashDestination) { Start-GraftHashStage 'Destination'; return }
        if ($script:RunContext.HashSource -and $script:RunContext.HashDestination) { Show-GraftHashVerification }
        elseif ($script:SourceHashFatal -or $script:DestinationHashFatal -or $script:SourceHashFailures.Count -gt 0 -or $script:DestinationHashFailures.Count -gt 0) { $script:Outcome = 'HashFailed' }
        else { $script:Outcome = 'CompletedWithHashes' }
        Complete-GraftOperation
    }
    catch { Complete-GraftInternalFailure 'Hash completion' $_ }
}

function Show-GraftHashVerification {
    $verification = Compare-GraftHashes $script:SourceHashes $script:DestinationHashes
    $hasFailures = $script:SourceHashFatal -or $script:DestinationHashFatal -or $script:SourceHashFailures.Count -gt 0 -or $script:DestinationHashFailures.Count -gt 0
    $failed = $hasFailures -or $verification.Mismatched.Count -gt 0 -or $verification.Missing.Count -gt 0 -or $verification.Extra.Count -gt 0
    Add-GraftConsoleLine ''
    Add-GraftConsoleLine '>>> Hash Verification Report:' 'Command'
    if (-not $failed) {
        $script:Outcome = 'Verified'
        Add-GraftConsoleLine 'PASSED: All files matched perfectly.' 'Success'
        Add-GraftLog 'Hash verification: All files matched perfectly.'
        return
    }
    $script:Outcome = 'VerificationFailed'
    if ($hasFailures) { Add-GraftConsoleLine 'FAILED: Some files or directories could not be hashed.' 'Error' }
    else { Add-GraftConsoleLine 'FAILED: Source and destination verification did not match.' 'Error' }
    Add-GraftConsoleLine ("Summary: Matched: {0}, Mismatched: {1}, Missing in destination: {2}, Extra in destination: {3}" -f $verification.Matched.Count, $verification.Mismatched.Count, $verification.Missing.Count, $verification.Extra.Count) 'Summary'
    if ($verification.Matched.Count -gt 0) { Add-GraftConsoleLine "Success: Matched: $($verification.Matched.Count) files" 'Success' }
    if ($verification.Mismatched.Count -gt 0) {
        Add-GraftConsoleLine "FAILED: Mismatched: $($verification.Mismatched.Count) files" 'Error'
        foreach ($item in @($verification.Mismatched | Select-Object -First 25)) { Add-GraftConsoleLine ("  {0} | Source: {1} | Dest: {2}" -f $item.Path, ([string]$item.SourceHash).Substring(0, 16), ([string]$item.DestinationHash).Substring(0, 16)) 'Error' }
        if ($verification.Mismatched.Count -gt 25) { Add-GraftConsoleLine "  ... $($verification.Mismatched.Count - 25) additional mismatches omitted from the live console" 'Error' }
    }
    if ($verification.Missing.Count -gt 0) {
        Add-GraftConsoleLine "Warning: Missing in destination: $($verification.Missing.Count) files" 'Warning'
        foreach ($path in @($verification.Missing | Select-Object -First 25)) { Add-GraftConsoleLine "  $path" 'Warning' }
    }
    if ($verification.Extra.Count -gt 0) {
        Add-GraftConsoleLine "Warning: Extra in destination: $($verification.Extra.Count) files" 'Warning'
        foreach ($path in @($verification.Extra | Select-Object -First 25)) { Add-GraftConsoleLine "  $path" 'Warning' }
    }
    Add-GraftLog ("Hash verification: Matched: {0}, Mismatched: {1}, Missing: {2}, Extra: {3}, Source hash failures: {4}, Destination hash failures: {5}" -f $verification.Matched.Count, $verification.Mismatched.Count, $verification.Missing.Count, $verification.Extra.Count, $script:SourceHashFailures.Count, $script:DestinationHashFailures.Count)
}

function Get-GraftFileStatusEntries {
    $entries = New-Object 'System.Collections.Generic.List[object]'
    foreach ($item in $script:CapturedFileEntries) { $entries.Add($item) }
    if ($script:CapturedFileEntriesOmitted -gt 0) { $entries.Add([pscustomobject]@{ Status = 'Summary'; Path = "$($script:CapturedFileEntriesOmitted) additional file-status rows omitted; see raw Robocopy output below." }) }
    if ($entries.Count -eq 0 -and $script:TransferStats.robocopy_exit_code -eq 0 -and $script:TransferStats.files_total -gt 0 -and $script:TransferStats.files_copied -eq 0 -and $script:TransferStats.files_mismatch -eq 0 -and $script:TransferStats.files_failed -eq 0 -and $null -ne $script:RunContext) {
        if ($script:RunContext.SourceMode -eq 'File') {
            $entries.Add([pscustomobject]@{ Status = 'Already Synced'; Path = [System.IO.Path]::GetFileName($script:RunContext.Source) })
        }
        else {
            if ($script:SourceHashes.Count -gt 0) {
                foreach ($hash in @($script:SourceHashes | Select-Object -First 5000)) { $entries.Add([pscustomobject]@{ Status = 'Already Synced'; Path = [string]$hash.RelativePath }) }
                if ($script:SourceHashes.Count -gt 5000) { $entries.Add([pscustomobject]@{ Status = 'Summary'; Path = "$($script:SourceHashes.Count - 5000) additional already-synced paths are available in the source hash report." }) }
            }
            else { $entries.Add([pscustomobject]@{ Status = 'Already Synced'; Path = "$($script:TransferStats.files_total) files; enable Source File Hash for a per-file assured inventory." }) }
        }
    }
    return $entries.ToArray()
}

function Get-GraftLogContent {
    $context = $script:RunContext
    if ($null -eq $context) {
        Sync-GraftOptionsFromControls
        $context = [pscustomobject]@{
            Source = [string]$script:Controls.SourcePathBox.Text
            Destination = [string]$script:Controls.DestinationPathBox.Text
            SourceMode = [string]$script:Controls.SourceModeCombo.SelectedItem.Content
            Ticket = [string]$script:Controls.TicketBox.Text
            Options = ConvertTo-GraftOptions $script:Options
            Command = Get-GraftCommandPreview $script:Options ([string]$script:Controls.SourcePathBox.Text) ([string]$script:Controls.DestinationPathBox.Text) ([string]$script:Controls.SourceModeCombo.SelectedItem.Content)
        }
    }
    $lines = New-Object 'System.Collections.Generic.List[string]'
    $lines.Add('=== TRANSFER SUMMARY ===')
    $lines.Add('Date: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
    if (-not [string]::IsNullOrWhiteSpace($env:USERNAME)) { $lines.Add("Username: $env:USERNAME") }
    if (-not [string]::IsNullOrWhiteSpace([string]$context.Ticket)) { $lines.Add("AFT Ticket Number: $($context.Ticket)") }
    $lines.Add("Source: $($context.Source)")
    $lines.Add("Destination: $($context.Destination)")
    $lines.Add("Command: $($context.Command)")
    $lines.Add("Robocopy Exit Code: $($script:TransferStats.robocopy_exit_code)")
    $lines.Add("Outcome: $($script:Outcome)")
    $lines.Add('')
    $lines.Add('=== TRANSFER STATISTICS ===')
    $lines.Add(('Total Files:      {0}' -f $script:TransferStats.files_total))
    $lines.Add(('Files Copied:     {0}' -f $script:TransferStats.files_copied))
    $lines.Add(('Files Skipped:    {0}' -f $script:TransferStats.files_skipped))
    $lines.Add(('Files Mismatched: {0}' -f $script:TransferStats.files_mismatch))
    $lines.Add(('Files FAILED:     {0}' -f $script:TransferStats.files_failed))
    $lines.Add(('Files Extras:     {0}' -f $script:TransferStats.files_extras))
    $lines.Add('')
    $lines.Add(('Total Dirs:       {0}' -f $script:TransferStats.dirs_total))
    $lines.Add(('Dirs Copied:      {0}' -f $script:TransferStats.dirs_copied))
    $lines.Add(('Dirs Skipped:     {0}' -f $script:TransferStats.dirs_skipped))
    $lines.Add(('Dirs FAILED:      {0}' -f $script:TransferStats.dirs_failed))
    $lines.Add(('Dirs Extras:      {0}' -f $script:TransferStats.dirs_extras))
    if (-not [string]::IsNullOrWhiteSpace($script:TransferStats.bytes_total)) { $lines.Add("Total Bytes:      $($script:TransferStats.bytes_total)"); $lines.Add("Bytes Copied:     $($script:TransferStats.bytes_copied)") }
    if (-not [string]::IsNullOrWhiteSpace($script:TransferStats.bytes_failed)) { $lines.Add("Bytes FAILED:     $($script:TransferStats.bytes_failed)") }
    if (-not [string]::IsNullOrWhiteSpace($script:TransferStats.speed)) { $lines.Add("Transfer Speed:   $($script:TransferStats.speed)") }
    $lines.Add('')
    $lines.Add('=== FILE LIST ===')
    $fileEntries = @(Get-GraftFileStatusEntries)
    if ($fileEntries.Count -eq 0) { $lines.Add('No file-level status entries were captured for this run.') }
    else {
        $lines.Add('Status | File')
        $lines.Add('-------|-----')
        foreach ($item in $fileEntries) { $lines.Add("$($item.Status) | $($item.Path)") }
    }
    $lines.Add('')
    if ($script:SourceHashes.Count -gt 0) {
        $lines.Add('=== SOURCE FILE HASHES ===')
        $lines.Add('File Hash Report')
        $lines.Add('================')
        $lines.Add('')
        foreach ($hash in $script:SourceHashes) {
            $lines.Add([string]$hash.RelativePath)
            $lines.Add("  SHA-256: $($hash.Hash)")
            $lines.Add("  Size: $($hash.Size) bytes")
            $lines.Add('')
        }
    }
    if ($script:DestinationHashes.Count -gt 0) {
        $lines.Add('=== DESTINATION FILE HASHES ===')
        foreach ($hash in $script:DestinationHashes) { $lines.Add("$($hash.RelativePath) | SHA-256: $($hash.Hash) | $($hash.Size) bytes") }
        $lines.Add('')
    }
    $lines.Add('=== ROBOCOPY AND VERIFICATION OUTPUT ===')
    $lines.AddRange($script:AllConsoleLines.ToArray())
    $lines.Add('')
    $lines.Add('=== DETAILED LOG ===')
    $lines.AddRange($script:LogEntries.ToArray())
    $lines.Add('')
    return [string]::Join([Environment]::NewLine, $lines.ToArray())
}

function Save-GraftCurrentLogToHistory {
    if ($null -eq $script:RunContext) { return }
    $entry = if ($null -eq $script:CurrentEntryId) { $null } else { Get-GraftHistoryEntryById $script:CurrentEntryId }
    $content = Get-GraftLogContent
    $ticket = if ([string]::IsNullOrWhiteSpace($script:RunContext.Ticket)) { $null } else { $script:RunContext.Ticket }
    $timestamp = $script:RunContext.StartedAt
    if ($null -ne $entry) {
        $entry.log_content = $content
        $entry.ticket_number = $ticket
        $entry.outcome = $script:Outcome
        try { $timestamp = [DateTimeOffset]::Parse([string]$entry.timestamp) } catch { }
    }
    $fileName = Get-GraftLogFileName $timestamp ([string]$ticket)
    $path = Get-GraftUniqueLogPath $fileName
    try {
        Write-GraftUtf8File $path $content
        if ($null -ne $entry) { $entry.log_path = $path; $entry.log_content = $null }
        Add-GraftConsoleLine ">>> Log saved to: $path" 'Command'
        Add-GraftLog "Log saved to: $path"
        if ($null -eq $entry) { Add-GraftConsoleLine 'Warning: The run history entry was missing; the standalone log was preserved.' 'Warning' }
    }
    catch {
        Add-GraftConsoleLine ">>> Failed to auto-save log: $($_.Exception.Message)" 'Error'
        Add-GraftLog "Failed to auto-save log: $($_.Exception.Message)"
    }
    try { Save-GraftHistory } catch { Add-GraftConsoleLine "Warning: History could not be saved: $($_.Exception.Message)" 'Warning' }
    Update-GraftHistoryLists
}

function Complete-GraftOperation {
    try { Save-GraftCurrentLogToHistory }
    catch {
        try { Add-GraftConsoleLine "Error: Final log persistence failed: $($_.Exception.Message)" 'Error' } catch { }
    }
    finally {
        $script:State = 'Idle'
        $script:HashStage = $null
        $script:HashRunner = $null
        $script:ProcessRunner = $null
        $script:ValidationRunner = $null
        try {
            Set-GraftBusyUi $false
            $status = switch ($script:Outcome) {
                'Cancelled' { 'Cancelled' }
                'CopyFailed' { "Copy failed (exit $($script:TransferStats.robocopy_exit_code))" }
                'VerificationFailed' { 'Verification FAILED' }
                'Verified' { 'Verification passed' }
                'HashFailed' { 'Hashing completed with errors' }
                'InternalError' { 'Internal error - operation stopped safely' }
                'CompletedWithHashes' { 'Completed with hashes' }
                default { "Completed (exit $($script:TransferStats.robocopy_exit_code))" }
            }
            Set-GraftStatus $status
            $script:Controls.StatusText.Foreground = if ($script:Outcome -in @('CopyFailed', 'VerificationFailed', 'HashFailed', 'InternalError')) { [Windows.Media.Brushes]::Tomato } elseif ($script:Outcome -in @('Verified', 'Completed', 'CompletedWithHashes')) { [Windows.Media.Brushes]::LightGreen } else { [Windows.Media.Brushes]::Gainsboro }
            Update-GraftCommandPreview
            Update-GraftHistoryButtons
        }
        catch { }
        if ($script:CloseWhenIdle -and $null -ne $script:Window) {
            $script:CloseWhenIdle = $false
            $script:Window.Close()
        }
    }
}

function Export-GraftLogContent {
    param([string]$Content, [string]$DefaultFileName)
    $dialog = New-Object Microsoft.Win32.SaveFileDialog
    $dialog.Title = 'Export GRAFT Log'
    $dialog.FileName = $DefaultFileName
    $dialog.Filter = 'Log files (*.log)|*.log|Text files (*.txt)|*.txt|All files (*.*)|*.*'
    if ($dialog.ShowDialog($script:Window)) {
        try { Write-GraftUtf8File $dialog.FileName $Content }
        catch { [void][Windows.MessageBox]::Show($script:Window, "The log could not be saved:`n$($_.Exception.Message)", 'Export Log', 'OK', 'Error') }
    }
}

function Export-GraftCurrentLog {
    $ticket = if ($script:Controls.ContainsKey('TicketBox')) { [string]$script:Controls.TicketBox.Text } else { '' }
    Export-GraftLogContent (Get-GraftLogContent) (Get-GraftLogFileName ([DateTimeOffset]::Now) $ticket)
}

function Set-GraftSelectedHistoryEntry {
    param($SelectedItem, [ValidateSet('Saved', 'Recent')][string]$List)
    if ($null -eq $SelectedItem) { return }
    $script:HistorySelection = [string]$SelectedItem.Id
    if ($List -eq 'Saved') { $script:Controls.RecentHistoryList.SelectedItem = $null }
    else { $script:Controls.SavedHistoryList.SelectedItem = $null }
    Update-GraftHistoryButtons
}

function Load-GraftHistorySelection {
    param([switch]$Run)
    $entry = Get-GraftSelectedHistoryEntry
    if ($null -eq $entry) { return }
    $script:SourcePath = [string]$entry.source
    $script:DestinationPath = [string]$entry.destination
    $savedMode = [string](Get-GraftProperty $entry 'source_mode' '')
    $script:SourceMode = if ($savedMode -in @('Folder', 'File')) { $savedMode } else { 'Folder' }
    $script:Options = ConvertTo-GraftOptions $entry.options
    $script:UpdatingControls = $true
    try {
        $script:Controls.SourcePathBox.Text = $script:SourcePath
        $script:Controls.DestinationPathBox.Text = $script:DestinationPath
        foreach ($item in $script:Controls.SourceModeCombo.Items) { if ([string]$item.Content -eq $script:SourceMode) { $script:Controls.SourceModeCombo.SelectedItem = $item; break } }
    }
    finally { $script:UpdatingControls = $false }
    Update-GraftOptionControls
    $script:Controls.MainTabs.SelectedItem = $script:Controls.OptionsTab
    if ($Run) { Invoke-GraftRequestRun }
}

function Toggle-GraftHistorySave {
    $entry = Get-GraftSelectedHistoryEntry
    if ($null -eq $entry) { return }
    $entry.saved = -not [bool]$entry.saved
    Save-GraftHistory
    Update-GraftHistoryLists
}

function Show-GraftTextInputDialog {
    param([string]$Title, [string]$Prompt, [AllowEmptyString()][string]$InitialValue)
    $dialog = New-Object Windows.Window
    $dialog.Title = $Title
    $dialog.Owner = $script:Window
    $dialog.Width = 470
    $dialog.Height = 165
    $dialog.ResizeMode = 'NoResize'
    $dialog.WindowStartupLocation = 'CenterOwner'
    $dialog.ShowInTaskbar = $false
    $dialog.Background = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(28, 27, 31))
    $dialog.Foreground = [Windows.Media.Brushes]::Gainsboro
    $grid = New-Object Windows.Controls.Grid
    $grid.Margin = New-Object Windows.Thickness(14)
    [void]$grid.RowDefinitions.Add((New-Object Windows.Controls.RowDefinition -Property @{ Height = 'Auto' }))
    [void]$grid.RowDefinitions.Add((New-Object Windows.Controls.RowDefinition -Property @{ Height = 'Auto' }))
    [void]$grid.RowDefinitions.Add((New-Object Windows.Controls.RowDefinition -Property @{ Height = '*' }))
    $promptBlock = New-Object Windows.Controls.TextBlock
    $promptBlock.Text = $Prompt
    $promptBlock.TextWrapping = 'Wrap'
    $promptBlock.Margin = New-Object Windows.Thickness(0, 0, 0, 8)
    [Windows.Controls.Grid]::SetRow($promptBlock, 0)
    [void]$grid.Children.Add($promptBlock)
    $input = New-Object Windows.Controls.TextBox
    $input.Text = $InitialValue
    $input.Padding = New-Object Windows.Thickness(6, 4, 6, 4)
    $input.Background = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(33, 31, 38))
    $input.Foreground = [Windows.Media.Brushes]::Gainsboro
    $input.BorderBrush = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(73, 69, 79))
    [Windows.Controls.Grid]::SetRow($input, 1)
    [void]$grid.Children.Add($input)
    $buttons = New-Object Windows.Controls.StackPanel
    $buttons.Orientation = 'Horizontal'
    $buttons.HorizontalAlignment = 'Right'
    $buttons.VerticalAlignment = 'Bottom'
    $ok = New-Object Windows.Controls.Button
    $ok.Content = 'OK'; $ok.Width = 78; $ok.Margin = New-Object Windows.Thickness(4); $ok.IsDefault = $true
    $cancel = New-Object Windows.Controls.Button
    $cancel.Content = 'Cancel'; $cancel.Width = 78; $cancel.Margin = New-Object Windows.Thickness(4); $cancel.IsCancel = $true
    [void]$buttons.Children.Add($ok); [void]$buttons.Children.Add($cancel)
    [Windows.Controls.Grid]::SetRow($buttons, 2)
    [void]$grid.Children.Add($buttons)
    $dialog.Content = $grid
    $ok.Add_Click({ $dialog.DialogResult = $true })
    $dialog.Add_ContentRendered({ $input.Focus() | Out-Null; $input.SelectAll() })
    $accepted = $dialog.ShowDialog()
    return [pscustomobject]@{ Accepted = [bool]$accepted; Text = [string]$input.Text }
}

function Rename-GraftHistorySelection {
    $entry = Get-GraftSelectedHistoryEntry
    if ($null -eq $entry) { return }
    $currentName = if ($null -eq $entry.name) { '' } else { [string]$entry.name }
    $result = Show-GraftTextInputDialog 'Rename History Entry' 'Enter a name. Leave it blank to restore the automatic source/destination name.' $currentName
    if (-not $result.Accepted) { return }
    $name = [string]$result.Text
    $entry.name = if ([string]::IsNullOrWhiteSpace($name)) { $null } else { $name }
    Save-GraftHistory
    Update-GraftHistoryLists
}

function Delete-GraftHistorySelection {
    $entry = Get-GraftSelectedHistoryEntry
    if ($null -eq $entry) { return }
    $answer = [Windows.MessageBox]::Show($script:Window, 'Delete the selected history entry? The separately saved log file will not be removed.', 'Delete History Entry', 'YesNo', 'Question')
    if ($answer -ne [Windows.MessageBoxResult]::Yes) { return }
    $script:History.entries = @($script:History.entries | Where-Object { [string]$_.id -ne [string]$entry.id })
    Save-GraftHistory
    Update-GraftHistoryLists
}

function Export-GraftHistorySelection {
    $entry = Get-GraftSelectedHistoryEntry
    if ($null -eq $entry) { return }
    $content = $entry.log_content
    if ($null -eq $content -and -not [string]::IsNullOrWhiteSpace([string]$entry.log_path) -and (Test-Path -LiteralPath $entry.log_path)) { $content = Get-Content -LiteralPath $entry.log_path -Raw }
    if ($null -eq $content) { return }
    try { $stamp = [DateTimeOffset]::Parse([string]$entry.timestamp) } catch { $stamp = [DateTimeOffset]::Now }
    Export-GraftLogContent ([string]$content) (Get-GraftLogFileName $stamp ([string]$entry.ticket_number))
}

function Clear-GraftUnsavedHistory {
    if (@($script:History.entries | Where-Object { -not $_.saved }).Count -eq 0) { return }
    $answer = [Windows.MessageBox]::Show($script:Window, 'Clear all unsaved recent command entries?', 'Clear Recent History', 'YesNo', 'Question')
    if ($answer -ne [Windows.MessageBoxResult]::Yes) { return }
    $script:History.entries = @($script:History.entries | Where-Object saved)
    Save-GraftHistory
    Update-GraftHistoryLists
}

function Select-GraftFolder {
    param([string]$Description, [string]$InitialPath)
    $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
    $dialog.Description = $Description
    $dialog.ShowNewFolderButton = $true
    if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { return $dialog.SelectedPath }
    return $null
}

function Invoke-GraftTimerTick {
    try {
        if ($null -ne $script:ValidationRunner) { Receive-GraftPathValidation }
        if ($null -ne $script:ProcessRunner) { Receive-GraftProcessOutput }
        if ($null -ne $script:HashRunner) { Receive-GraftHashOutput }
    }
    catch {
        $failure = $_
        Complete-GraftInternalFailure 'Dispatcher update' $failure
    }
}

$script:Xaml = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="GRAFT - Graphical Robocopy Assured File Transfer Tool"
        Width="1400" Height="900" MinWidth="1000" MinHeight="700"
        WindowStartupLocation="CenterScreen" Background="#1C1B1F"
        Foreground="#E6E1E5" FontFamily="Segoe UI" FontSize="13">
    <Window.Resources>
        <SolidColorBrush x:Key="Surface" Color="#1C1B1F"/>
        <SolidColorBrush x:Key="Surface2" Color="#211F26"/>
        <SolidColorBrush x:Key="Surface3" Color="#2B2930"/>
        <SolidColorBrush x:Key="Surface4" Color="#36343B"/>
        <SolidColorBrush x:Key="TextPrimary" Color="#E6E1E5"/>
        <SolidColorBrush x:Key="TextSecondary" Color="#CAC4D0"/>
        <SolidColorBrush x:Key="Accent" Color="#4FC3F7"/>
        <Style TargetType="TextBlock">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="VerticalAlignment" Value="Center"/>
        </Style>
        <Style TargetType="Label">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="VerticalContentAlignment" Value="Center"/>
        </Style>
        <Style TargetType="Button">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="Background" Value="{StaticResource Surface4}"/>
            <Setter Property="BorderBrush" Value="#49454F"/>
            <Setter Property="Padding" Value="14,7"/>
            <Setter Property="Margin" Value="3"/>
            <Setter Property="Cursor" Value="Hand"/>
            <Style.Triggers>
                <Trigger Property="IsMouseOver" Value="True"><Setter Property="BorderBrush" Value="{StaticResource Accent}"/><Setter Property="Background" Value="#414047"/></Trigger>
                <Trigger Property="IsEnabled" Value="False"><Setter Property="Opacity" Value="0.45"/></Trigger>
            </Style.Triggers>
        </Style>
        <Style TargetType="TextBox">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="Background" Value="{StaticResource Surface2}"/>
            <Setter Property="BorderBrush" Value="#49454F"/>
            <Setter Property="CaretBrush" Value="{StaticResource Accent}"/>
            <Setter Property="Padding" Value="7,5"/>
            <Setter Property="Margin" Value="3"/>
        </Style>
        <Style TargetType="ComboBox">
            <Setter Property="Foreground" Value="#111111"/>
            <Setter Property="Background" Value="{StaticResource Surface4}"/>
            <Setter Property="Padding" Value="6,4"/>
            <Setter Property="Margin" Value="3"/>
        </Style>
        <Style TargetType="CheckBox">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="Margin" Value="3"/>
            <Setter Property="VerticalAlignment" Value="Center"/>
        </Style>
        <Style TargetType="Expander">
            <Setter Property="Foreground" Value="{StaticResource TextPrimary}"/>
            <Setter Property="Margin" Value="4,4,8,4"/>
        </Style>
        <Style TargetType="TabControl">
            <Setter Property="Background" Value="{StaticResource Surface}"/>
            <Setter Property="BorderBrush" Value="#49454F"/>
        </Style>
        <Style TargetType="TabItem">
            <Setter Property="Foreground" Value="{StaticResource TextSecondary}"/>
            <Setter Property="Background" Value="{StaticResource Surface2}"/>
            <Setter Property="BorderBrush" Value="#49454F"/>
            <Setter Property="Padding" Value="16,8"/>
            <Setter Property="Cursor" Value="Hand"/>
            <Setter Property="Template">
                <Setter.Value>
                    <ControlTemplate TargetType="TabItem">
                        <Border x:Name="TabBorder"
                                Background="{TemplateBinding Background}"
                                BorderBrush="{TemplateBinding BorderBrush}"
                                BorderThickness="1,1,1,0"
                                CornerRadius="3,3,0,0"
                                Margin="0,0,3,0"
                                Padding="{TemplateBinding Padding}">
                            <ContentPresenter ContentSource="Header"
                                              HorizontalAlignment="Center"
                                              VerticalAlignment="Center"
                                              RecognizesAccessKey="True"
                                              TextElement.Foreground="{TemplateBinding Foreground}"/>
                        </Border>
                    </ControlTemplate>
                </Setter.Value>
            </Setter>
            <Style.Triggers>
                <Trigger Property="IsMouseOver" Value="True">
                    <Setter Property="Background" Value="{StaticResource Surface4}"/>
                    <Setter Property="Foreground" Value="#FFFFFF"/>
                </Trigger>
                <Trigger Property="IsSelected" Value="True">
                    <Setter Property="Background" Value="{StaticResource Surface3}"/>
                    <Setter Property="Foreground" Value="#FFFFFF"/>
                    <Setter Property="BorderBrush" Value="{StaticResource Accent}"/>
                </Trigger>
                <Trigger Property="IsEnabled" Value="False">
                    <Setter Property="Opacity" Value="0.45"/>
                </Trigger>
            </Style.Triggers>
        </Style>
        <Style TargetType="ListView"><Setter Property="Foreground" Value="{StaticResource TextPrimary}"/><Setter Property="Background" Value="{StaticResource Surface2}"/><Setter Property="BorderBrush" Value="#343139"/></Style>
        <Style TargetType="ListViewItem"><Setter Property="Foreground" Value="{StaticResource TextPrimary}"/><Setter Property="Background" Value="Transparent"/><Setter Property="Padding" Value="2"/></Style>
        <Style TargetType="Menu"><Setter Property="Foreground" Value="{StaticResource TextPrimary}"/><Setter Property="Background" Value="#1F1E22"/></Style>
        <Style TargetType="MenuItem"><Setter Property="Foreground" Value="{StaticResource TextPrimary}"/><Setter Property="Background" Value="#1F1E22"/></Style>
    </Window.Resources>

    <Grid>
        <Grid.RowDefinitions>
            <RowDefinition Height="Auto"/>
            <RowDefinition Height="Auto"/>
            <RowDefinition Height="*"/>
            <RowDefinition Height="5"/>
            <RowDefinition Height="155" MinHeight="90"/>
        </Grid.RowDefinitions>

        <Menu Grid.Row="0" Height="29">
            <MenuItem Header="File">
                <MenuItem x:Name="MenuExportLog" Header="Export Log..."/>
                <Separator/>
                <MenuItem x:Name="MenuExit" Header="Exit"/>
            </MenuItem>
            <MenuItem Header="Help">
                <MenuItem x:Name="MenuAbout" Header="About"/>
            </MenuItem>
        </Menu>

        <Border Grid.Row="1" Background="{StaticResource Surface}" BorderBrush="#343139" BorderThickness="0,0,0,1" Padding="12,8">
            <StackPanel>
                <TextBlock Text="GRAFT - Graphical Robocopy Assured File Transfer Tool" FontSize="22" Margin="0,0,0,9"/>
                <Grid x:Name="ConfigurationPanel">
                    <Grid.RowDefinitions>
                        <RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/>
                    </Grid.RowDefinitions>
                    <Grid.ColumnDefinitions>
                        <ColumnDefinition Width="95"/><ColumnDefinition Width="Auto"/><ColumnDefinition Width="Auto"/><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/>
                    </Grid.ColumnDefinitions>

                    <TextBlock Grid.Row="0" Grid.Column="0" Text="Source:"/>
                    <ComboBox x:Name="SourceModeCombo" Grid.Row="0" Grid.Column="1" Width="82"/>
                    <Button x:Name="BrowseSourceButton" Grid.Row="0" Grid.Column="2" Content="Browse..."/>
                    <ComboBox x:Name="RecentSourceCombo" Grid.Row="0" Grid.Column="3" Width="150"/>
                    <TextBox x:Name="SourcePathBox" Grid.Row="0" Grid.Column="4" ToolTip="Select a source folder or file, or type its path."/>

                    <TextBlock Grid.Row="1" Grid.Column="0" Text="Destination:"/>
                    <Button x:Name="BrowseDestinationButton" Grid.Row="1" Grid.Column="2" Content="Browse..."/>
                    <ComboBox x:Name="RecentDestinationCombo" Grid.Row="1" Grid.Column="3" Width="150"/>
                    <TextBox x:Name="DestinationPathBox" Grid.Row="1" Grid.Column="4" ToolTip="Select or type a destination folder."/>

                    <TextBlock Grid.Row="2" Grid.Column="0" Text="AFT Ticket:"/>
                    <TextBox x:Name="TicketBox" Grid.Row="2" Grid.Column="1" Grid.ColumnSpan="2" Width="180" HorizontalAlignment="Left" ToolTip="Optional ticket number included in history and log names."/>

                    <TextBlock Grid.Row="3" Grid.Column="0" Text="Command:"/>
                    <TextBox x:Name="CommandPreview" Grid.Row="3" Grid.Column="1" Grid.ColumnSpan="4" IsReadOnly="True" FontFamily="Consolas"/>

                    <StackPanel Grid.Row="4" Grid.Column="1" Grid.ColumnSpan="4" Orientation="Horizontal" Margin="0,3,0,2">
                        <CheckBox x:Name="HashSourceCheck" Content="Include Source File Hash (SHA-256)" IsChecked="True"/>
                        <CheckBox x:Name="HashDestinationCheck" Content="Include Destination Hash Verification (SHA-256)" Margin="18,3,3,3"/>
                        <Button x:Name="HashInfoButton" Content="Info" Padding="8,3"/>
                    </StackPanel>
                </Grid>

                <Grid Margin="0,4,0,0">
                    <Grid.ColumnDefinitions><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/><ColumnDefinition Width="260"/></Grid.ColumnDefinitions>
                    <Button x:Name="RunButton" Grid.Column="0" Content="Run Robocopy" MinWidth="135"/>
                    <TextBlock x:Name="HashComparisonHint" Grid.Column="1" Text="Source and destination hashes will be compared automatically." Foreground="#80CBC4" Margin="12,0" Visibility="Collapsed"/>
                    <StackPanel Grid.Column="2" Orientation="Horizontal" HorizontalAlignment="Right">
                        <ProgressBar x:Name="StatusProgress" Width="110" Height="8" Visibility="Collapsed" Margin="0,0,10,0"/>
                        <TextBlock x:Name="StatusText" Text="Ready" MinWidth="120" TextAlignment="Right"/>
                    </StackPanel>
                </Grid>
            </StackPanel>
        </Border>

        <Grid Grid.Row="2">
            <Grid.ColumnDefinitions><ColumnDefinition Width="0.95*" MinWidth="410"/><ColumnDefinition Width="5"/><ColumnDefinition Width="1.05*" MinWidth="390"/></Grid.ColumnDefinitions>
            <TabControl x:Name="MainTabs" Grid.Column="0" Margin="8,7,3,4" SelectedIndex="0">
                <TabItem x:Name="OptionsTab" Header="Options">
                    <ScrollViewer VerticalScrollBarVisibility="Auto" HorizontalScrollBarVisibility="Disabled">
                        <StackPanel Margin="8">
                            <TextBlock Text="Presets" FontSize="20" Margin="0,0,0,5"/>
                            <StackPanel Orientation="Horizontal">
                                <ComboBox x:Name="PresetCombo" Width="270"/>
                            </StackPanel>
                            <TextBlock x:Name="PresetDescription" Foreground="{StaticResource TextSecondary}" TextWrapping="Wrap" Margin="4,2,4,10"/>
                            <TextBlock x:Name="DestructiveBanner" Foreground="#FF7850" FontWeight="Bold" TextWrapping="Wrap" Margin="4,3,4,7" Visibility="Collapsed"/>
                            <Border Background="{StaticResource Surface2}" BorderBrush="#343139" BorderThickness="1" Padding="7" Margin="2,3,6,8">
                                <StackPanel x:Name="DryRunHost"/>
                            </Border>
                            <Grid>
                                <Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="*"/></Grid.ColumnDefinitions>
                                <StackPanel Grid.Column="0">
                                    <Expander Header="Copy Options" IsExpanded="True"><StackPanel x:Name="CopyOptionsHost" Margin="8,5"/></Expander>
                                    <Expander Header="File Selection"><StackPanel x:Name="FileSelectionHost" Margin="8,5"/></Expander>
                                    <Expander Header="Attributes"><StackPanel x:Name="AttributesHost" Margin="8,5"/></Expander>
                                </StackPanel>
                                <StackPanel Grid.Column="1">
                                    <Expander Header="Retry Options" IsExpanded="True"><StackPanel x:Name="RetryOptionsHost" Margin="8,5"/></Expander>
                                    <Expander Header="Logging Options"><StackPanel x:Name="LoggingOptionsHost" Margin="8,5"/></Expander>
                                    <Expander Header="File Filters"><StackPanel x:Name="FileFiltersHost" Margin="8,5"/></Expander>
                                    <Expander Header="Performance" IsExpanded="True"><StackPanel x:Name="PerformanceHost" Margin="8,5"/></Expander>
                                </StackPanel>
                            </Grid>
                        </StackPanel>
                    </ScrollViewer>
                </TabItem>

                <TabItem x:Name="HistoryTab" Header="History">
                    <Grid Margin="8">
                        <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/><RowDefinition Height="Auto"/></Grid.RowDefinitions>
                        <DockPanel Grid.Row="0" Margin="0,0,0,5">
                            <Button x:Name="ClearUnsavedButton" DockPanel.Dock="Right" Content="Clear Unsaved"/>
                            <TextBlock Text="Command History" FontSize="20"/>
                        </DockPanel>
                        <TextBlock Grid.Row="1" Text="Saved Commands" FontWeight="Bold" Margin="2,3"/>
                        <ListView x:Name="SavedHistoryList" Grid.Row="2">
                            <ListView.View><GridView><GridViewColumn Header="Name" DisplayMemberBinding="{Binding Name}" Width="260"/><GridViewColumn Header="Command" DisplayMemberBinding="{Binding Command}" Width="310"/><GridViewColumn Header="Source" DisplayMemberBinding="{Binding Source}" Width="160"/><GridViewColumn Header="Destination" DisplayMemberBinding="{Binding Destination}" Width="160"/><GridViewColumn Header="Log" DisplayMemberBinding="{Binding HasLog}" Width="45"/></GridView></ListView.View>
                        </ListView>
                        <TextBlock Grid.Row="3" Text="Recent Commands" FontWeight="Bold" Margin="2,8,2,3"/>
                        <ListView x:Name="RecentHistoryList" Grid.Row="4">
                            <ListView.View><GridView><GridViewColumn Header="Name" DisplayMemberBinding="{Binding Name}" Width="260"/><GridViewColumn Header="Command" DisplayMemberBinding="{Binding Command}" Width="310"/><GridViewColumn Header="Source" DisplayMemberBinding="{Binding Source}" Width="160"/><GridViewColumn Header="Destination" DisplayMemberBinding="{Binding Destination}" Width="160"/><GridViewColumn Header="Log" DisplayMemberBinding="{Binding HasLog}" Width="45"/></GridView></ListView.View>
                        </ListView>
                        <WrapPanel x:Name="HistoryActionPanel" Grid.Row="5" Margin="0,6,0,0">
                            <Button x:Name="HistoryLoadButton" Content="Load"/><Button x:Name="HistoryRunButton" Content="Run"/><Button x:Name="HistorySaveButton" Content="Save"/><Button x:Name="HistoryRenameButton" Content="Rename"/><Button x:Name="HistoryExportButton" Content="Export Log"/><Button x:Name="HistoryDeleteButton" Content="Delete"/>
                        </WrapPanel>
                    </Grid>
                </TabItem>
            </TabControl>

            <GridSplitter Grid.Column="1" Width="5" HorizontalAlignment="Stretch" Background="#343139"/>
            <Grid Grid.Column="2" Margin="4,7,8,4">
                <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                <DockPanel Grid.Row="0" Margin="0,0,0,5">
                    <Button x:Name="ClearConsoleButton" DockPanel.Dock="Right" Content="Clear"/>
                    <TextBlock Text="Console Output" FontSize="20"/>
                </DockPanel>
                <RichTextBox x:Name="ConsoleBox" Grid.Row="1" IsReadOnly="True" Background="{StaticResource Surface}" Foreground="{StaticResource TextPrimary}" BorderBrush="#343139" VerticalScrollBarVisibility="Auto" FontFamily="Consolas" Padding="6"/>
            </Grid>
        </Grid>

        <GridSplitter Grid.Row="3" Height="5" HorizontalAlignment="Stretch" Background="#343139"/>
        <Grid Grid.Row="4" Margin="8,3,8,7">
            <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
            <DockPanel Grid.Row="0" Margin="0,0,0,4">
                <StackPanel DockPanel.Dock="Right" Orientation="Horizontal"><Button x:Name="ClearLogButton" Content="Clear"/><Button x:Name="ExportLogButton" Content="Export Log"/></StackPanel>
                <TextBlock Text="Log" FontSize="20"/>
            </DockPanel>
            <TextBox x:Name="LogBox" Grid.Row="1" IsReadOnly="True" TextWrapping="NoWrap" FontFamily="Consolas" FontSize="11" VerticalScrollBarVisibility="Auto" HorizontalScrollBarVisibility="Auto"/>
        </Grid>
    </Grid>
</Window>
'@

function Update-GraftOptionErrorVisuals {
    foreach ($definition in $script:OptionDefinitions | Where-Object HasValue) {
        if ($script:OptionControls.ContainsKey($definition.Key)) {
            $script:OptionControls[$definition.Key].Error.Text = ''
            $script:OptionControls[$definition.Key].Error.Visibility = 'Collapsed'
        }
    }
    foreach ($error in @(Get-GraftOptionValidationErrors $script:Options)) {
        foreach ($definition in $script:OptionDefinitions | Where-Object HasValue) {
            if ($error.StartsWith($definition.Name + ':', [StringComparison]::Ordinal)) {
                $script:OptionControls[$definition.Key].Error.Text = 'Warning: ' + $error.Substring($definition.Name.Length + 1).Trim()
                $script:OptionControls[$definition.Key].Error.Visibility = 'Visible'
                break
            }
        }
    }
}

function Add-GraftOptionControls {
    $hostMap = @{
        'Copy Options' = 'CopyOptionsHost'; 'File Selection' = 'FileSelectionHost'; 'Attributes' = 'AttributesHost'
        'Retry Options' = 'RetryOptionsHost'; 'Logging Options' = 'LoggingOptionsHost'; 'File Filters' = 'FileFiltersHost'
        'Performance' = 'PerformanceHost'; 'Special' = 'DryRunHost'
    }
    foreach ($definition in $script:OptionDefinitions) {
        $container = New-Object Windows.Controls.StackPanel
        $container.Margin = New-Object Windows.Thickness(0, 2, 0, 7)
        $top = New-Object Windows.Controls.DockPanel
        $check = New-Object Windows.Controls.CheckBox
        $check.Content = $definition.Name
        $check.Tag = $definition.Key
        $check.ToolTip = $definition.Flag
        [Windows.Controls.DockPanel]::SetDock($check, [Windows.Controls.Dock]::Left)
        [void]$top.Children.Add($check)
        $valueBox = $null
        if ($definition.HasValue) {
            $valueBox = New-Object Windows.Controls.TextBox
            $valueBox.Width = 76
            $valueBox.Tag = $definition.Key
            $valueBox.Margin = New-Object Windows.Thickness(8, 1, 2, 1)
            $valueBox.HorizontalAlignment = 'Left'
            [void]$top.Children.Add($valueBox)
        }
        $description = New-Object Windows.Controls.TextBlock
        $description.Text = $definition.Description
        $description.Foreground = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(176, 170, 182))
        $description.FontSize = 11
        $description.TextWrapping = 'Wrap'
        $description.Margin = New-Object Windows.Thickness(5, 1, 2, 0)
        $errorText = New-Object Windows.Controls.TextBlock
        $errorText.Foreground = [Windows.Media.Brushes]::Orange
        $errorText.FontSize = 11
        $errorText.TextWrapping = 'Wrap'
        $errorText.Margin = New-Object Windows.Thickness(5, 1, 2, 0)
        $errorText.Visibility = 'Collapsed'
        [void]$container.Children.Add($top)
        [void]$container.Children.Add($description)
        [void]$container.Children.Add($errorText)
        $script:OptionControls[$definition.Key] = [pscustomobject]@{ Check = $check; Value = $valueBox; Error = $errorText }

        $changeHandler = {
            if ($script:UpdatingControls) { return }
            $key = [string]$this.Tag
            if ($null -ne $script:OptionControls[$key].Value) { $script:OptionControls[$key].Value.IsEnabled = [bool]$this.IsChecked }
            Update-GraftPresetAfterManualChange
            Update-GraftOptionErrorVisuals
        }
        $check.Add_Checked($changeHandler)
        $check.Add_Unchecked($changeHandler)
        if ($definition.HasValue) {
            $valueBox.Add_TextChanged({
                if ($script:UpdatingControls) { return }
                Update-GraftPresetAfterManualChange
                Update-GraftOptionErrorVisuals
            })
        }
        [void]$script:Controls[$hostMap[$definition.Category]].Children.Add($container)
    }
}

function Show-GraftDestinationHashWarning {
    param([bool]$CanDisable)
    if ($SelfTest -or $SmokeTest) {
        $both = [bool]$script:Controls.HashSourceCheck.IsChecked -and [bool]$script:Controls.HashDestinationCheck.IsChecked
        $script:Controls.HashComparisonHint.Visibility = if ($both) { 'Visible' } else { 'Collapsed' }
        return
    }
    $message = @"
Destination hashing reads every destination file after the transfer and may take considerable time on a slow network.

When source hashing is also enabled, GRAFT compares all SHA-256 hashes and reports matched, mismatched, missing, extra, and unreadable files.

Typical throughput varies widely:
- Local drive: roughly 100-500 MB/s
- Gigabit LAN: roughly 50-100 MB/s
- WAN or slow network: 1-10 MB/s or slower

Continue with destination hash verification?
"@
    $answer = [Windows.MessageBox]::Show($script:Window, $message.Trim(), 'Destination Hash Verification', 'YesNo', 'Warning')
    if ($CanDisable -and $answer -ne [Windows.MessageBoxResult]::Yes) {
        $script:UpdatingControls = $true
        try { $script:Controls.HashDestinationCheck.IsChecked = $false }
        finally { $script:UpdatingControls = $false }
    }
    $both = [bool]$script:Controls.HashSourceCheck.IsChecked -and [bool]$script:Controls.HashDestinationCheck.IsChecked
    $script:Controls.HashComparisonHint.Visibility = if ($both) { 'Visible' } else { 'Collapsed' }
}

function Set-GraftWindowIcon {
    param([Windows.Window]$Window)
    $drawing = New-Object Windows.Media.DrawingGroup
    $backgroundBrush = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(28, 27, 31))
    $backgroundGeometry = New-Object Windows.Media.RectangleGeometry((New-Object Windows.Rect(0, 0, 64, 64)))
    [void]$drawing.Children.Add((New-Object Windows.Media.GeometryDrawing($backgroundBrush, $null, $backgroundGeometry)))
    $accentBrush = New-Object Windows.Media.SolidColorBrush([Windows.Media.Color]::FromRgb(79, 195, 247))
    $pen = New-Object Windows.Media.Pen($accentBrush, 7)
    $pen.StartLineCap = 'Round'; $pen.EndLineCap = 'Round'; $pen.LineJoin = 'Round'
    $geometry = [Windows.Media.Geometry]::Parse('M 49,19 A 23,23 0 1 0 49,45 M 36,32 L 53,32 L 53,44')
    [void]$drawing.Children.Add((New-Object Windows.Media.GeometryDrawing($null, $pen, $geometry)))
    $Window.Icon = New-Object Windows.Media.DrawingImage($drawing)
}

function Initialize-GraftWindow {
    [xml]$xamlDocument = $script:Xaml
    $reader = New-Object System.Xml.XmlNodeReader($xamlDocument)
    $script:Window = [Windows.Markup.XamlReader]::Load($reader)
    Set-GraftWindowIcon $script:Window
    $controlNames = @(
        'MenuExportLog','MenuExit','MenuAbout','ConfigurationPanel','SourceModeCombo','BrowseSourceButton','RecentSourceCombo','SourcePathBox',
        'BrowseDestinationButton','RecentDestinationCombo','DestinationPathBox','TicketBox','CommandPreview','HashSourceCheck','HashDestinationCheck',
        'HashInfoButton','RunButton','HashComparisonHint','StatusProgress','StatusText','MainTabs','OptionsTab','HistoryTab','PresetCombo',
        'PresetDescription','DestructiveBanner','DryRunHost','CopyOptionsHost','FileSelectionHost','AttributesHost','RetryOptionsHost','LoggingOptionsHost',
        'FileFiltersHost','PerformanceHost','ClearUnsavedButton','SavedHistoryList','RecentHistoryList','HistoryActionPanel','HistoryLoadButton',
        'HistoryRunButton','HistorySaveButton','HistoryRenameButton','HistoryExportButton','HistoryDeleteButton','ClearConsoleButton','ConsoleBox',
        'ClearLogButton','ExportLogButton','LogBox'
    )
    foreach ($name in $controlNames) {
        $control = $script:Window.FindName($name)
        if ($null -eq $control) { throw "The WPF layout is missing the '$name' control." }
        $script:Controls[$name] = $control
    }

    foreach ($mode in @('Folder', 'File')) {
        $item = New-Object Windows.Controls.ComboBoxItem
        $item.Content = $mode
        [void]$script:Controls.SourceModeCombo.Items.Add($item)
    }
    foreach ($preset in $script:PresetOrder) {
        $item = New-Object Windows.Controls.ComboBoxItem
        $item.Content = $script:PresetDefinitions[$preset].Name
        $item.Tag = $preset
        [void]$script:Controls.PresetCombo.Items.Add($item)
    }
    Add-GraftOptionControls

    $script:UpdatingControls = $true
    try {
        $script:Controls.SourcePathBox.Text = $script:SourcePath
        $script:Controls.DestinationPathBox.Text = $script:DestinationPath
        foreach ($item in $script:Controls.SourceModeCombo.Items) { if ([string]$item.Content -eq $script:SourceMode) { $script:Controls.SourceModeCombo.SelectedItem = $item; break } }
    }
    finally { $script:UpdatingControls = $false }
    Update-GraftOptionControls
    Update-GraftOptionErrorVisuals
    Update-GraftRecentCombos
    Update-GraftHistoryLists

    $script:Controls.SourcePathBox.Add_TextChanged({ if (-not $script:UpdatingControls) { Update-GraftCommandPreview } })
    $script:Controls.DestinationPathBox.Add_TextChanged({ if (-not $script:UpdatingControls) { Update-GraftCommandPreview } })
    $script:Controls.SourceModeCombo.Add_SelectionChanged({ if (-not $script:UpdatingControls -and $null -ne $this.SelectedItem) { $script:SourceMode = [string]$this.SelectedItem.Content; Update-GraftCommandPreview } })
    $script:Controls.TicketBox.Add_TextChanged({ })
    $script:Controls.PresetCombo.Add_SelectionChanged({
        if ($script:UpdatingControls -or $null -eq $this.SelectedItem) { return }
        Set-GraftPreset $script:Options ([string]$this.SelectedItem.Tag)
        Update-GraftOptionControls
        Update-GraftOptionErrorVisuals
    })
    $script:Controls.BrowseSourceButton.Add_Click({
        if ([string]$script:Controls.SourceModeCombo.SelectedItem.Content -eq 'File') {
            $dialog = New-Object Microsoft.Win32.OpenFileDialog
            $dialog.Title = 'Select Source File'
            $dialog.CheckFileExists = $true
            if ($dialog.ShowDialog($script:Window)) { $script:Controls.SourcePathBox.Text = $dialog.FileName }
        }
        else {
            $selected = Select-GraftFolder 'Select Source Folder' ([string]$script:Controls.SourcePathBox.Text)
            if ($null -ne $selected) { $script:Controls.SourcePathBox.Text = $selected }
        }
    })
    $script:Controls.BrowseDestinationButton.Add_Click({
        $selected = Select-GraftFolder 'Select Destination Folder' ([string]$script:Controls.DestinationPathBox.Text)
        if ($null -ne $selected) { $script:Controls.DestinationPathBox.Text = $selected }
    })
    $script:Controls.RecentSourceCombo.Add_SelectionChanged({
        if ($script:UpdatingControls -or $this.SelectedIndex -le 0) { return }
        $script:Controls.SourcePathBox.Text = [string]$this.SelectedItem
        $detectedMode = Get-GraftStoredSourceMode ([string]$this.SelectedItem)
        if ($null -ne $detectedMode) {
            foreach ($item in $script:Controls.SourceModeCombo.Items) { if ([string]$item.Content -eq $detectedMode) { $script:Controls.SourceModeCombo.SelectedItem = $item; break } }
        }
        $this.SelectedIndex = 0
    })
    $script:Controls.RecentDestinationCombo.Add_SelectionChanged({
        if ($script:UpdatingControls -or $this.SelectedIndex -le 0) { return }
        $script:Controls.DestinationPathBox.Text = [string]$this.SelectedItem
        $this.SelectedIndex = 0
    })
    $script:Controls.RunButton.Add_Click({ Invoke-GraftRequestRun })
    $script:Controls.HashDestinationCheck.Add_Checked({ if (-not $script:UpdatingControls) { Show-GraftDestinationHashWarning $true } })
    $script:Controls.HashDestinationCheck.Add_Unchecked({ if (-not $script:UpdatingControls) { $script:Controls.HashComparisonHint.Visibility = 'Collapsed' } })
    $script:Controls.HashSourceCheck.Add_Checked({ if ([bool]$script:Controls.HashDestinationCheck.IsChecked) { $script:Controls.HashComparisonHint.Visibility = 'Visible' } })
    $script:Controls.HashSourceCheck.Add_Unchecked({ $script:Controls.HashComparisonHint.Visibility = 'Collapsed' })
    $script:Controls.HashInfoButton.Add_Click({ Show-GraftDestinationHashWarning $false })
    $script:Controls.ClearConsoleButton.Add_Click({ Clear-GraftConsole })
    $script:Controls.ClearLogButton.Add_Click({ Clear-GraftLog })
    $script:Controls.ExportLogButton.Add_Click({ Export-GraftCurrentLog })
    $script:Controls.MenuExportLog.Add_Click({ Export-GraftCurrentLog })
    $script:Controls.MenuExit.Add_Click({ $script:Window.Close() })
    $script:Controls.MenuAbout.Add_Click({ [void][Windows.MessageBox]::Show($script:Window, "GRAFT`nGraphical Robocopy Assured File Transfer Tool`n`nPowerShell GUI version $($script:AppVersion)`nNative Windows 11 / Windows PowerShell 5.1", 'About GRAFT', 'OK', 'Information') })

    $script:Controls.SavedHistoryList.Add_SelectionChanged({
        if ($null -ne $this.SelectedItem) { Set-GraftSelectedHistoryEntry $this.SelectedItem 'Saved' }
        elseif ($null -eq $script:Controls.RecentHistoryList.SelectedItem) { $script:HistorySelection = $null; Update-GraftHistoryButtons }
    })
    $script:Controls.RecentHistoryList.Add_SelectionChanged({
        if ($null -ne $this.SelectedItem) { Set-GraftSelectedHistoryEntry $this.SelectedItem 'Recent' }
        elseif ($null -eq $script:Controls.SavedHistoryList.SelectedItem) { $script:HistorySelection = $null; Update-GraftHistoryButtons }
    })
    $script:Controls.SavedHistoryList.Add_MouseDoubleClick({ if ($null -ne $this.SelectedItem) { Rename-GraftHistorySelection } })
    $script:Controls.RecentHistoryList.Add_MouseDoubleClick({ if ($null -ne $this.SelectedItem) { Rename-GraftHistorySelection } })
    $script:Controls.HistoryLoadButton.Add_Click({ Load-GraftHistorySelection })
    $script:Controls.HistoryRunButton.Add_Click({ Load-GraftHistorySelection -Run })
    $script:Controls.HistorySaveButton.Add_Click({ Toggle-GraftHistorySave })
    $script:Controls.HistoryRenameButton.Add_Click({ Rename-GraftHistorySelection })
    $script:Controls.HistoryExportButton.Add_Click({ Export-GraftHistorySelection })
    $script:Controls.HistoryDeleteButton.Add_Click({ Delete-GraftHistorySelection })
    $script:Controls.ClearUnsavedButton.Add_Click({ Clear-GraftUnsavedHistory })

    $script:Window.Add_Closing({
        param($sender, $eventArgs)
        if ($script:State -eq 'Idle') { return }
        $eventArgs.Cancel = $true
        if ($script:CloseWhenIdle) {
            $forceAnswer = [Windows.MessageBox]::Show($script:Window, 'GRAFT is still waiting for a worker to stop. Force close now? The final audit log may be incomplete.', 'Force Close GRAFT', 'YesNo', 'Warning')
            if ($forceAnswer -eq [Windows.MessageBoxResult]::Yes) {
                try { if ($null -ne $script:ProcessRunner) { $script:ProcessRunner.Cancel() } } catch { }
                try { if ($null -ne $script:HashRunner) { $script:HashRunner.Cancel() } } catch { }
                $script:CloseWhenIdle = $false
                $script:State = 'Idle'
                $eventArgs.Cancel = $false
            }
            return
        }
        $answer = [Windows.MessageBox]::Show($script:Window, 'An operation is still active. Cancel it and close GRAFT?', 'Close GRAFT', 'YesNo', 'Warning')
        if ($answer -ne [Windows.MessageBoxResult]::Yes) { return }
        $script:CloseWhenIdle = $true
        Invoke-GraftCancel
        if ($script:State -eq 'Idle') { $script:CloseWhenIdle = $false; $eventArgs.Cancel = $false }
    })

    $script:DispatcherTimer = New-Object Windows.Threading.DispatcherTimer
    $script:DispatcherTimer.Interval = [TimeSpan]::FromMilliseconds(90)
    $script:DispatcherTimer.Add_Tick({ Invoke-GraftTimerTick })
    $script:DispatcherTimer.Start()
    Update-GraftCommandPreview
    Set-GraftStatus 'Ready'
    return $script:Window
}

function Invoke-GraftSelfTest {
    $assertions = 0
    function Assert-Graft([bool]$Condition, [string]$Message) {
        if (-not $Condition) { throw "Self-test failed: $Message" }
        $script:SelfTestAssertions++
    }
    $script:SelfTestAssertions = 0
    $options = New-GraftOptions 'LargeFilesWan'
    $actual = @(Get-GraftArguments $options 'C:\Source' 'D:\Destination' 'Folder')
    $expected = @('C:\Source','D:\Destination','/E','/J','/COPY:DAT','/DCOPY:DAT','/R:3','/W:5','/NP','/MT:8','/XJ','/UNICODE')
    Assert-Graft (($actual -join '|') -eq ($expected -join '|')) 'Large Files over WAN arguments do not match the Rust preset.'
    $presetExpectations = [ordered]@{
        MirrorWithMetadata = @('/Z','/COPY:DATS','/MIR','/R:3','/W:5','/MT:8','/XJ','/UNICODE')
        CopyAllPreserve = @('/E','/Z','/COPY:DATS','/R:3','/W:5','/MT:8','/XJ','/UNICODE')
        IncrementalBackup = @('/E','/COPY:DAT','/R:3','/W:5','/XO','/MT:8','/XJ','/UNICODE')
        QuickCopy = @('/E','/R:1','/W:1','/MT:16','/XJ','/UNICODE')
    }
    foreach ($presetName in $presetExpectations.Keys) {
        $presetArguments = @(Get-GraftArguments (New-GraftOptions $presetName) 'C:\Source' 'D:\Destination' 'Folder')
        Assert-Graft (($presetArguments[2..($presetArguments.Count - 1)] -join '|') -eq ($presetExpectations[$presetName] -join '|')) "$presetName arguments do not match the Rust preset."
    }
    $fileArgs = @(Get-GraftArguments (New-GraftOptions 'QuickCopy') 'C:\Source Folder\one file.bin' 'D:\Destination Folder' 'File')
    Assert-Graft ($fileArgs[0] -eq 'C:\Source Folder' -and $fileArgs[2] -eq 'one file.bin') 'Single-file source was not split into parent and filter.'
    $preview = Get-GraftCommandPreview (New-GraftOptions 'QuickCopy') 'C:\Source Folder' 'D:\Destination Folder' 'Folder'
    Assert-Graft ($preview.Contains('"C:\Source Folder"') -and $preview.Contains('/MT:16')) 'Command preview quoting or options are incorrect.'
    $invalid = New-GraftOptions
    Enable-GraftOption $invalid 'multi_thread' '129'
    Assert-Graft (@(Get-GraftOptionValidationErrors $invalid).Count -eq 1) 'MT range validation failed.'
    $invalid.copy_flags.enabled = $true
    $invalid.copy_flags.value = 'DZX'
    Assert-Graft (@(Get-GraftOptionValidationErrors $invalid).Count -eq 2) 'COPY flag validation failed.'
    $spacedValue = New-GraftOptions
    Enable-GraftOption $spacedValue 'retry_count' ' 3 '
    $spacedArguments = @(Get-GraftArguments $spacedValue 'C:\Source' 'D:\Destination' 'Folder')
    Assert-Graft (@(Get-GraftOptionValidationErrors $spacedValue).Count -eq 0 -and $spacedArguments -contains '/R:3' -and $spacedArguments -notcontains '/R: 3 ') 'Option values were not normalized before execution.'
    $moveDryRun = New-GraftOptions
    Enable-GraftOption $moveDryRun 'move_files'
    Assert-Graft (Test-GraftMoveSourceHashConflict $moveDryRun $true) 'Move/source-hash incompatibility was not detected.'
    Enable-GraftOption $moveDryRun 'dry_run'
    Assert-Graft (-not (Test-GraftMoveSourceHashConflict $moveDryRun $true)) 'Dry Run incorrectly conflicts with move and source hashing.'
    $driveRoot = [System.IO.Path]::GetPathRoot($script:DataRoot)
    Assert-Graft ([string]::Equals((Get-GraftComparablePath $driveRoot), $driveRoot, [StringComparison]::OrdinalIgnoreCase)) 'Drive-root normalization changed the root into a drive-relative path.'

    $script:ConsoleLines.Clear()
    $script:ConsoleLines.Add([pscustomobject]@{ Text = '    Dirs :         5         3         2         0         0         1'; Kind = 'Summary' })
    $script:ConsoleLines.Add([pscustomobject]@{ Text = '   Files :        10         8         2         0         0         0'; Kind = 'Summary' })
    $script:ConsoleLines.Add([pscustomobject]@{ Text = '   Bytes :   1.234 m   1.001 m   233.0 k         0         0         0'; Kind = 'Summary' })
    Update-GraftTransferStats 3
    Assert-Graft ($script:TransferStats.robocopy_exit_code -eq 3 -and $script:TransferStats.files_copied -eq 8 -and $script:TransferStats.dirs_extras -eq 1) 'Robocopy statistics parsing failed.'
    Clear-GraftConsole
    $outputBatch = New-Object 'System.Collections.Generic.List[object]'
    for ($batchIndex = 0; $batchIndex -lt 2601; $batchIndex++) { $outputBatch.Add([pscustomobject]@{ Text = "line-$batchIndex"; Kind = 'Normal' }) }
    Add-GraftConsoleBatch $outputBatch.ToArray()
    Assert-Graft ($script:AllConsoleLines.Count -eq 2601 -and $script:ConsoleLines.Count -eq 2000) 'Complete output retention or bounded live-console behavior failed.'
    Clear-GraftConsole

    $testFile = Join-Path $script:DataRoot 'abc.bin'
    [System.IO.File]::WriteAllBytes($testFile, [byte[]](97,98,99))
    $hashRunner = [Graft.Native.HashRunner]::StartFile($testFile, 'abc.bin')
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $hashRunner.IsCompleted -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 10 }
    $hashes = @($hashRunner.GetResults())
    Assert-Graft ($hashes.Count -eq 1 -and $hashes[0].Hash -eq 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') 'SHA-256 hashing failed.'
    $different = [Graft.Native.HashRecord]::new()
    $different.RelativePath = 'abc.bin'; $different.Hash = ('0' * 64); $different.Size = 3
    $comparison = Compare-GraftHashes $hashes @($different)
    Assert-Graft ($comparison.Mismatched.Count -eq 1) 'Hash mismatch detection failed.'
    $extra = [Graft.Native.HashRecord]::new()
    $extra.RelativePath = 'extra.bin'; $extra.Hash = ('1' * 64); $extra.Size = 1
    $comparison = Compare-GraftHashes $hashes @($hashes[0], $extra)
    Assert-Graft ($comparison.Extra.Count -eq 1) 'Extra destination file detection failed.'
    $sameHashDifferentCase = [Graft.Native.HashRecord]::new()
    $sameHashDifferentCase.RelativePath = 'ABC.BIN'; $sameHashDifferentCase.Hash = $hashes[0].Hash; $sameHashDifferentCase.Size = 3
    $comparison = Compare-GraftHashes $hashes @($sameHashDifferentCase)
    Assert-Graft ($comparison.Matched.Count -eq 1 -and $comparison.Missing.Count -eq 0 -and $comparison.Extra.Count -eq 0) 'Windows case-insensitive hash-path comparison failed.'

    $cancelHashFile = Join-Path $script:DataRoot 'cancel-hash.bin'
    $cancelHashStream = [System.IO.File]::Open($cancelHashFile, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try { $cancelHashStream.SetLength(64MB) } finally { $cancelHashStream.Dispose() }
    $cancelHashRunner = [Graft.Native.HashRunner]::StartFile($cancelHashFile, 'cancel-hash.bin')
    $cancelHashRunner.Cancel()
    $cancelHashDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $cancelHashRunner.IsCompleted -and [DateTime]::UtcNow -lt $cancelHashDeadline) { Start-Sleep -Milliseconds 10 }
    Assert-Graft ($cancelHashRunner.IsCompleted -and $cancelHashRunner.CancelRequested) 'Hash worker cancellation did not complete.'

    $pingPath = Join-Path $env:SystemRoot 'System32\PING.EXE'
    $cancelProcessRunner = [Graft.Native.ProcessRunner]::Start($pingPath, [string[]]@('127.0.0.1', '-n', '30', '-w', '1000'))
    Start-Sleep -Milliseconds 100
    $cancelProcessRunner.Cancel()
    $cancelProcessDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $cancelProcessRunner.IsCompleted -and [DateTime]::UtcNow -lt $cancelProcessDeadline) { Start-Sleep -Milliseconds 20 }
    Assert-Graft ($cancelProcessRunner.IsCompleted -and $cancelProcessRunner.CancelRequested) 'Process worker cancellation did not complete.'

    $copySource = Join-Path $script:DataRoot 'copy source'
    $copyDestination = Join-Path $script:DataRoot 'copy destination'
    [System.IO.Directory]::CreateDirectory($copySource) | Out-Null
    [System.IO.Directory]::CreateDirectory($copyDestination) | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $copySource 'sample file.bin'), [byte[]](1,2,3,4,5))
    $robocopyPath = Join-Path $env:SystemRoot 'System32\robocopy.exe'
    $copyRunner = [Graft.Native.ProcessRunner]::Start($robocopyPath, [string[]]@($copySource, $copyDestination, 'sample file.bin', '/R:0', '/W:0', '/NP', '/NFL', '/NDL'))
    $copyDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not $copyRunner.IsCompleted -and [DateTime]::UtcNow -lt $copyDeadline) { Start-Sleep -Milliseconds 20 }
    Assert-Graft ($copyRunner.IsCompleted -and $copyRunner.ExitCode -ge 0 -and $copyRunner.ExitCode -lt 8 -and (Test-Path -LiteralPath (Join-Path $copyDestination 'sample file.bin') -PathType Leaf)) 'Isolated Robocopy process execution failed.'
    $unicodeName = 'caf' + [char]0x00E9 + '-' + [char]0x6F22 + [char]0x5B57 + '.txt'
    [System.IO.File]::WriteAllText((Join-Path $copySource $unicodeName), 'unicode')
    $unicodeRunner = [Graft.Native.ProcessRunner]::Start($robocopyPath, [string[]]@($copySource, $copyDestination, $unicodeName, '/L', '/FP', '/NJH', '/NJS', '/NP', '/NDL', '/R:0', '/W:0', '/UNICODE'))
    $unicodeDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not $unicodeRunner.IsCompleted -and [DateTime]::UtcNow -lt $unicodeDeadline) { Start-Sleep -Milliseconds 20 }
    $unicodeLines = New-Object 'System.Collections.Generic.List[string]'
    $unicodeMessage = $null
    while ($unicodeRunner.Messages.TryDequeue([ref]$unicodeMessage)) {
        $unicodeLines.Add([string]$unicodeMessage.Text)
        $unicodeMessage = $null
    }
    Assert-Graft ($unicodeRunner.IsCompleted -and (($unicodeLines -join "`n").Contains($unicodeName))) 'Robocopy Unicode output did not preserve the exact filename.'
    $insideSource = Join-Path $copySource 'nested destination'
    [System.IO.Directory]::CreateDirectory($insideSource) | Out-Null
    Assert-Graft ($null -ne (Test-GraftPaths $copySource $insideSource 'Folder' (New-GraftOptions 'QuickCopy'))) 'Nested destination validation failed.'
    $pathValidation = [Graft.Native.PathValidationRunner]::Start($copySource, $copyDestination, 'Folder', $false, $false, $false)
    $pathValidationDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $pathValidation.IsCompleted -and [DateTime]::UtcNow -lt $pathValidationDeadline) { Start-Sleep -Milliseconds 10 }
    Assert-Graft ($pathValidation.IsCompleted -and [string]::IsNullOrWhiteSpace([string]$pathValidation.Error)) 'Background path validation rejected valid local paths.'
    $nestedValidation = [Graft.Native.PathValidationRunner]::Start($copySource, $insideSource, 'Folder', $false, $false, $false)
    $nestedValidationDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not $nestedValidation.IsCompleted -and [DateTime]::UtcNow -lt $nestedValidationDeadline) { Start-Sleep -Milliseconds 10 }
    Assert-Graft ($nestedValidation.IsCompleted -and -not [string]::IsNullOrWhiteSpace([string]$nestedValidation.Error)) 'Background path validation missed a nested destination.'

    $legacy = [pscustomobject]@{ timestamp = [DateTime]::Now.ToString('o'); source = $copySource; destination = $copyDestination; command = 'robocopy'; preset = 'Quick Copy (No Extras)'; dryRun = $true }
    $migrated = ConvertTo-GraftHistoryEntry $legacy
    Assert-Graft ($migrated.options.current_preset -eq 'None' -and $migrated.options.dry_run.enabled) 'Legacy PowerShell history migration failed.'
    $script:History = New-GraftEmptyHistory
    $historyContext = [pscustomobject]@{ StartedAt = [DateTimeOffset]::Now; Source = $testFile; SourceMode = 'File'; Destination = $copyDestination; Command = 'robocopy test'; Options = New-GraftOptions 'QuickCopy'; Ticket = 'SELFTEST' }
    Add-GraftHistoryEntry (New-GraftHistoryEntry $historyContext)
    Save-GraftHistory
    $roundTrip = Import-GraftHistory
    Assert-Graft ($roundTrip.entries.Count -eq 1 -and $roundTrip.entries[0].options.current_preset -eq 'QuickCopy' -and $roundTrip.entries[0].source_mode -eq 'File') 'History JSON round trip or source-mode persistence failed.'

    $window = Initialize-GraftWindow
    Assert-Graft ($null -ne $window -and $script:OptionControls.Count -eq $script:OptionDefinitions.Count) 'WPF layout or dynamic options did not initialize.'
    $window.UpdateLayout()
    $optionsTabBackground = [Windows.Media.SolidColorBrush]$script:Controls.OptionsTab.Background
    $optionsTabForeground = [Windows.Media.SolidColorBrush]$script:Controls.OptionsTab.Foreground
    Assert-Graft ($optionsTabBackground.Color.R -eq 43 -and $optionsTabBackground.Color.G -eq 41 -and $optionsTabBackground.Color.B -eq 48 -and $optionsTabForeground.Color.R -eq 255) 'The selected Options tab did not use the accessible dark color scheme.'
    $script:Controls.MainTabs.SelectedItem = $script:Controls.HistoryTab
    $window.UpdateLayout()
    $historyTabBackground = [Windows.Media.SolidColorBrush]$script:Controls.HistoryTab.Background
    $historyTabForeground = [Windows.Media.SolidColorBrush]$script:Controls.HistoryTab.Foreground
    Assert-Graft ($historyTabBackground.Color.R -eq 43 -and $historyTabBackground.Color.G -eq 41 -and $historyTabBackground.Color.B -eq 48 -and $historyTabForeground.Color.R -eq 255) 'The selected History tab did not use the accessible dark color scheme.'
    $script:Controls.MainTabs.SelectedItem = $script:Controls.OptionsTab
    $appSource = Join-Path $script:DataRoot 'app source'
    $appDestination = Join-Path $script:DataRoot 'app destination'
    [System.IO.Directory]::CreateDirectory($appSource) | Out-Null
    [System.IO.Directory]::CreateDirectory($appDestination) | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $appSource 'state-machine.bin'), [byte[]](9,8,7,6))
    $script:Options = New-GraftOptions 'QuickCopy'
    $script:SourcePath = $appSource; $script:DestinationPath = $appDestination; $script:SourceMode = 'Folder'
    $script:UpdatingControls = $true
    try {
        $script:Controls.SourcePathBox.Text = $appSource
        $script:Controls.DestinationPathBox.Text = $appDestination
        $script:Controls.HashSourceCheck.IsChecked = $false
        $script:Controls.HashDestinationCheck.IsChecked = $false
    }
    finally { $script:UpdatingControls = $false }
    Update-GraftOptionControls
    Invoke-GraftRequestRun
    $operationDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($script:State -ne 'Idle' -and [DateTime]::UtcNow -lt $operationDeadline) { Invoke-GraftTimerTick; Start-Sleep -Milliseconds 20 }
    Assert-Graft ($script:State -eq 'Idle' -and (Test-Path -LiteralPath (Join-Path $appDestination 'state-machine.bin') -PathType Leaf)) 'Application transfer state machine did not finish successfully.'
    $completedEntry = Get-GraftHistoryEntryById $script:CurrentEntryId
    Assert-Graft ($null -ne $completedEntry -and -not [string]::IsNullOrWhiteSpace([string]$completedEntry.log_path) -and (Test-Path -LiteralPath $completedEntry.log_path -PathType Leaf)) 'Automatic history log persistence failed.'

    $script:UpdatingControls = $true
    try {
        $script:Controls.HashSourceCheck.IsChecked = $true
        $script:Controls.HashDestinationCheck.IsChecked = $true
    }
    finally { $script:UpdatingControls = $false }
    Start-GraftTransfer
    $verificationDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($script:State -ne 'Idle' -and [DateTime]::UtcNow -lt $verificationDeadline) { Invoke-GraftTimerTick; Start-Sleep -Milliseconds 20 }
    $verifiedEntry = Get-GraftHistoryEntryById $script:CurrentEntryId
    Assert-Graft ($script:State -eq 'Idle' -and $script:Outcome -eq 'Verified' -and $script:SourceHashes.Count -eq 1 -and $script:DestinationHashes.Count -eq 1 -and $verifiedEntry.outcome -eq 'Verified') 'End-to-end source/destination verification did not complete successfully.'
    $script:State = 'Hashing'; $script:Outcome = 'Hashing'; $script:CancelRequested = $false
    Invoke-GraftCancel
    Assert-Graft ($script:State -eq 'Cancelling' -and $script:Outcome -eq 'Cancelled' -and $script:CancelRequested) 'Hash-stage cancellation did not retain the Cancelled outcome.'
    $script:State = 'Idle'; $script:CancelRequested = $false
    $script:DispatcherTimer.Stop()
    $window.Close()
    Write-Output "GRAFT self-test passed ($script:SelfTestAssertions assertions)."
}

function Remove-GraftOwnedSelfTestData {
    if (-not $script:OwnsSelfTestDataRoot) { return }
    $ownedPath = [System.IO.Path]::GetFullPath($script:DataRoot)
    $script:OwnsSelfTestDataRoot = $false
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $tempRoot.EndsWith([System.IO.Path]::DirectorySeparatorChar)) { $tempRoot += [System.IO.Path]::DirectorySeparatorChar }
    $leaf = [System.IO.Path]::GetFileName($ownedPath.TrimEnd([char]'\', [char]'/'))
    if ($ownedPath.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and $leaf.StartsWith('GraftSelfTest-', [StringComparison]::Ordinal)) {
        Remove-Item -LiteralPath $ownedPath -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($SelfTest) {
    try {
        Invoke-GraftSelfTest
        Remove-GraftOwnedSelfTestData
        exit 0
    }
    catch {
        Write-Error ("{0}`n{1}" -f $_.Exception.Message, $_.ScriptStackTrace)
        Remove-GraftOwnedSelfTestData
        exit 1
    }
}

$mainWindow = Initialize-GraftWindow
if (-not [string]::IsNullOrWhiteSpace([string]$script:StartupWarning)) { [void][Windows.MessageBox]::Show($mainWindow, $script:StartupWarning, 'GRAFT History Recovery', 'OK', 'Warning') }
if ($SmokeTest) {
    $smokeTimer = New-Object Windows.Threading.DispatcherTimer
    $smokeTimer.Interval = [TimeSpan]::FromMilliseconds(600)
    $smokeTimer.Add_Tick({ $this.Stop(); $script:Window.Close() })
    $smokeTimer.Start()
}
[void]$mainWindow.ShowDialog()
if ($null -ne $script:DispatcherTimer) { $script:DispatcherTimer.Stop() }
