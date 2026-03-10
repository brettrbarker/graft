//! File hashing module for verifying file transfers

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use walkdir::WalkDir;

/// Result of hashing a single file
#[derive(Clone, Debug)]
pub struct FileHash {
    #[allow(dead_code)] // Available for future use (e.g., opening files)
    pub path: PathBuf,
    pub relative_path: String,
    pub hash: String,
    pub size: u64,
}

/// Result of comparing source and destination hashes
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct HashVerification {
    pub matched: Vec<(String, String)>,       // (relative_path, hash)
    pub mismatched: Vec<(String, String, String)>, // (relative_path, source_hash, dest_hash)
    pub missing_in_dest: Vec<String>,
    pub extra_in_dest: Vec<String>,
}

#[allow(dead_code)]
impl HashVerification {
    pub fn is_successful(&self) -> bool {
        self.mismatched.is_empty() && self.missing_in_dest.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Matched: {}, Mismatched: {}, Missing in destination: {}, Extra in destination: {}",
            self.matched.len(),
            self.mismatched.len(),
            self.missing_in_dest.len(),
            self.extra_in_dest.len()
        )
    }
}

/// Progress update during hashing
#[derive(Clone, Debug)]
pub enum HashProgress {
    Starting(usize),           // Total files to hash
    FileStarted(String),       // File path
    FileComplete(FileHash),    // Completed file hash
    Complete(Vec<FileHash>),   // All hashes complete
    Error(String),             // Error message
}

/// Hash a single file using SHA-256
pub fn hash_file(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|e| format!("Failed to open file {}: {}", path.display(), e))?;
    
    let mut reader = BufReader::with_capacity(1024 * 1024, file); // 1MB buffer
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 64]; // 64KB chunks

    loop {
        let bytes_read = reader.read(&mut buffer)
            .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
        
        if bytes_read == 0 {
            break;
        }
        
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Get relative path from base
fn get_relative_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Collect all files in a directory
pub fn collect_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let mut files = Vec::new();
    
    for entry in WalkDir::new(dir).follow_links(true) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() {
                    files.push(entry.path().to_path_buf());
                }
            }
            Err(e) => {
                return Err(format!("Error walking directory: {}", e));
            }
        }
    }

    Ok(files)
}

/// Hash all files in a directory with progress updates
pub fn hash_directory(
    dir: &Path,
    progress_tx: mpsc::Sender<HashProgress>,
    cancel_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<Vec<FileHash>, String>> {
    let dir = dir.to_path_buf();
    
    thread::spawn(move || {
        let files = collect_files(&dir)?;
        let total = files.len();
        
        let _ = progress_tx.send(HashProgress::Starting(total));
        
        let mut hashes = Vec::new();
        
        for file_path in files {
            // Check for cancellation
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = progress_tx.send(HashProgress::Error("Cancelled by user".to_string()));
                return Err("Cancelled by user".to_string());
            }
            
            let relative = get_relative_path(&dir, &file_path);
            let _ = progress_tx.send(HashProgress::FileStarted(relative.clone()));
            
            match hash_file(&file_path) {
                Ok(hash) => {
                    let size = std::fs::metadata(&file_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    
                    let file_hash = FileHash {
                        path: file_path,
                        relative_path: relative,
                        hash,
                        size,
                    };
                    
                    let _ = progress_tx.send(HashProgress::FileComplete(file_hash.clone()));
                    hashes.push(file_hash);
                }
                Err(e) => {
                    let _ = progress_tx.send(HashProgress::Error(e.clone()));
                    return Err(e);
                }
            }
        }
        
        let _ = progress_tx.send(HashProgress::Complete(hashes.clone()));
        Ok(hashes)
    })
}

/// Compare source and destination hashes
#[allow(dead_code)]
pub fn compare_hashes(source_hashes: &[FileHash], dest_hashes: &[FileHash]) -> HashVerification {
    let source_map: HashMap<&str, &FileHash> = source_hashes
        .iter()
        .map(|h| (h.relative_path.as_str(), h))
        .collect();
    
    let dest_map: HashMap<&str, &FileHash> = dest_hashes
        .iter()
        .map(|h| (h.relative_path.as_str(), h))
        .collect();

    let mut matched = Vec::new();
    let mut mismatched = Vec::new();
    let mut missing_in_dest = Vec::new();
    let mut extra_in_dest = Vec::new();

    // Check source files
    for (rel_path, source_hash) in &source_map {
        if let Some(dest_hash) = dest_map.get(rel_path) {
            if source_hash.hash == dest_hash.hash {
                matched.push((rel_path.to_string(), source_hash.hash.clone()));
            } else {
                mismatched.push((
                    rel_path.to_string(),
                    source_hash.hash.clone(),
                    dest_hash.hash.clone(),
                ));
            }
        } else {
            missing_in_dest.push(rel_path.to_string());
        }
    }

    // Check for extra files in destination
    for rel_path in dest_map.keys() {
        if !source_map.contains_key(rel_path) {
            extra_in_dest.push(rel_path.to_string());
        }
    }

    HashVerification {
        matched,
        mismatched,
        missing_in_dest,
        extra_in_dest,
    }
}

/// Format hash results for logging
pub fn format_hash_results(hashes: &[FileHash]) -> String {
    let mut output = String::new();
    output.push_str("File Hash Report\n");
    output.push_str("================\n\n");
    
    for hash in hashes {
        output.push_str(&format!(
            "{}\n  SHA-256: {}\n  Size: {} bytes\n\n",
            hash.relative_path, hash.hash, hash.size
        ));
    }
    
    output
}

/// Format verification results for logging
#[allow(dead_code)]
pub fn format_verification_results(verification: &HashVerification) -> String {
    let mut output = String::new();
    output.push_str("Hash Verification Report\n");
    output.push_str("========================\n\n");
    
    output.push_str(&format!("Summary: {}\n\n", verification.summary()));
    
    if !verification.matched.is_empty() {
        output.push_str("✓ Matched Files:\n");
        for (path, hash) in &verification.matched {
            output.push_str(&format!("  {} [{}]\n", path, &hash[..16]));
        }
        output.push('\n');
    }
    
    if !verification.mismatched.is_empty() {
        output.push_str("✗ Mismatched Files:\n");
        for (path, src, dst) in &verification.mismatched {
            output.push_str(&format!(
                "  {}\n    Source: {}\n    Dest:   {}\n",
                path, src, dst
            ));
        }
        output.push('\n');
    }
    
    if !verification.missing_in_dest.is_empty() {
        output.push_str("! Missing in Destination:\n");
        for path in &verification.missing_in_dest {
            output.push_str(&format!("  {}\n", path));
        }
        output.push('\n');
    }
    
    if !verification.extra_in_dest.is_empty() {
        output.push_str("+ Extra in Destination:\n");
        for path in &verification.extra_in_dest {
            output.push_str(&format!("  {}\n", path));
        }
        output.push('\n');
    }
    
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_temp_dir(test_name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join(format!(
            "roboaft_test_{}_{}_{}",
            std::process::id(),
            test_name,
            counter
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        temp_dir
    }

    fn cleanup_temp_dir(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn test_hash_file() {
        let temp_dir = create_temp_dir("hash_file");
        let test_file = temp_dir.join("test.txt");
        
        fs::write(&test_file, "Hello, World!").expect("Failed to write test file");
        
        let hash = hash_file(&test_file).expect("Failed to hash file");
        
        // SHA-256 of "Hello, World!" is known
        assert_eq!(hash, "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f");
        
        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_hash_empty_file() {
        let temp_dir = create_temp_dir("hash_empty");
        let test_file = temp_dir.join("empty.txt");
        
        fs::write(&test_file, "").expect("Failed to write test file");
        
        let hash = hash_file(&test_file).expect("Failed to hash file");
        
        // SHA-256 of empty string
        assert_eq!(hash, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        
        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_hash_nonexistent_file() {
        let result = hash_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_files() {
        let temp_dir = create_temp_dir("collect_files");
        
        // Create nested structure
        let subdir = temp_dir.join("subdir");
        fs::create_dir_all(&subdir).unwrap();
        
        fs::write(temp_dir.join("file1.txt"), "content1").unwrap();
        fs::write(temp_dir.join("file2.txt"), "content2").unwrap();
        fs::write(subdir.join("file3.txt"), "content3").unwrap();
        
        let files = collect_files(&temp_dir).expect("Failed to collect files");
        
        assert_eq!(files.len(), 3);
        
        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_collect_files_empty_dir() {
        let temp_dir = create_temp_dir("collect_empty");
        
        let files = collect_files(&temp_dir).expect("Failed to collect files");
        assert_eq!(files.len(), 0);
        
        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_collect_files_nonexistent_dir() {
        let result = collect_files(Path::new("/nonexistent/directory"));
        assert!(result.is_err());
    }

    #[test]
    fn test_compare_hashes_identical() {
        let source = vec![
            FileHash {
                path: PathBuf::from("/src/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
            FileHash {
                path: PathBuf::from("/src/file2.txt"),
                relative_path: "file2.txt".to_string(),
                hash: "def456".to_string(),
                size: 200,
            },
        ];
        
        let dest = vec![
            FileHash {
                path: PathBuf::from("/dst/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
            FileHash {
                path: PathBuf::from("/dst/file2.txt"),
                relative_path: "file2.txt".to_string(),
                hash: "def456".to_string(),
                size: 200,
            },
        ];
        
        let result = compare_hashes(&source, &dest);
        
        assert!(result.is_successful());
        assert_eq!(result.matched.len(), 2);
        assert_eq!(result.mismatched.len(), 0);
        assert_eq!(result.missing_in_dest.len(), 0);
        assert_eq!(result.extra_in_dest.len(), 0);
    }

    #[test]
    fn test_compare_hashes_mismatch() {
        let source = vec![
            FileHash {
                path: PathBuf::from("/src/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
        ];
        
        let dest = vec![
            FileHash {
                path: PathBuf::from("/dst/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "different_hash".to_string(),
                size: 100,
            },
        ];
        
        let result = compare_hashes(&source, &dest);
        
        assert!(!result.is_successful());
        assert_eq!(result.matched.len(), 0);
        assert_eq!(result.mismatched.len(), 1);
        assert_eq!(result.mismatched[0].0, "file1.txt");
    }

    #[test]
    fn test_compare_hashes_missing_in_dest() {
        let source = vec![
            FileHash {
                path: PathBuf::from("/src/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
            FileHash {
                path: PathBuf::from("/src/file2.txt"),
                relative_path: "file2.txt".to_string(),
                hash: "def456".to_string(),
                size: 200,
            },
        ];
        
        let dest = vec![
            FileHash {
                path: PathBuf::from("/dst/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
        ];
        
        let result = compare_hashes(&source, &dest);
        
        assert!(!result.is_successful());
        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.missing_in_dest.len(), 1);
        assert!(result.missing_in_dest.contains(&"file2.txt".to_string()));
    }

    #[test]
    fn test_compare_hashes_extra_in_dest() {
        let source = vec![
            FileHash {
                path: PathBuf::from("/src/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
        ];
        
        let dest = vec![
            FileHash {
                path: PathBuf::from("/dst/file1.txt"),
                relative_path: "file1.txt".to_string(),
                hash: "abc123".to_string(),
                size: 100,
            },
            FileHash {
                path: PathBuf::from("/dst/extra.txt"),
                relative_path: "extra.txt".to_string(),
                hash: "extra_hash".to_string(),
                size: 50,
            },
        ];
        
        let result = compare_hashes(&source, &dest);
        
        // Extra files don't cause failure
        assert!(result.is_successful());
        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.extra_in_dest.len(), 1);
    }

    #[test]
    fn test_hash_verification_summary() {
        let verification = HashVerification {
            matched: vec![("file1.txt".to_string(), "hash1".to_string())],
            mismatched: vec![("file2.txt".to_string(), "src".to_string(), "dst".to_string())],
            missing_in_dest: vec!["file3.txt".to_string()],
            extra_in_dest: vec!["file4.txt".to_string()],
        };
        
        let summary = verification.summary();
        assert!(summary.contains("Matched: 1"));
        assert!(summary.contains("Mismatched: 1"));
        assert!(summary.contains("Missing in destination: 1"));
        assert!(summary.contains("Extra in destination: 1"));
    }

    #[test]
    fn test_format_hash_results() {
        let hashes = vec![
            FileHash {
                path: PathBuf::from("/path/file.txt"),
                relative_path: "file.txt".to_string(),
                hash: "abc123def456".to_string(),
                size: 1024,
            },
        ];
        
        let output = format_hash_results(&hashes);
        assert!(output.contains("file.txt"));
        assert!(output.contains("abc123def456"));
        assert!(output.contains("1024 bytes"));
    }

    #[test]
    fn test_format_verification_results() {
        let verification = HashVerification {
            matched: vec![("matched.txt".to_string(), "abc123def456abc1".to_string())],
            mismatched: vec![("bad.txt".to_string(), "src_hash".to_string(), "dst_hash".to_string())],
            missing_in_dest: vec!["missing.txt".to_string()],
            extra_in_dest: vec!["extra.txt".to_string()],
        };
        
        let output = format_verification_results(&verification);
        assert!(output.contains("matched.txt"));
        assert!(output.contains("bad.txt"));
        assert!(output.contains("missing.txt"));
        assert!(output.contains("extra.txt"));
        assert!(output.contains("✓ Matched"));
        assert!(output.contains("✗ Mismatched"));
    }

    #[test]
    fn test_hash_directory_integration() {
        let temp_dir = create_temp_dir("hash_dir_integration");
        
        // Create test files
        fs::write(temp_dir.join("file1.txt"), "content1").unwrap();
        fs::write(temp_dir.join("file2.txt"), "content2").unwrap();
        
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let handle = hash_directory(&temp_dir, tx, cancel_flag);
        
        // Collect progress updates
        let mut starting_count = 0;
        let mut file_complete_count = 0;
        let mut final_hashes = Vec::new();
        
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(HashProgress::Starting(n)) => starting_count = n,
                Ok(HashProgress::FileComplete(_)) => file_complete_count += 1,
                Ok(HashProgress::Complete(hashes)) => {
                    final_hashes = hashes;
                    break;
                }
                Ok(HashProgress::Error(e)) => panic!("Hash error: {}", e),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        
        handle.join().expect("Hash thread panicked").expect("Hash failed");
        
        assert_eq!(starting_count, 2);
        assert_eq!(file_complete_count, 2);
        assert_eq!(final_hashes.len(), 2);
        
        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn test_get_relative_path() {
        use super::get_relative_path;
        
        let base = Path::new("/home/user/documents");
        let file = Path::new("/home/user/documents/subfolder/file.txt");
        
        let relative = get_relative_path(base, file);
        
        // Path::strip_prefix returns paths with forward slashes on all platforms
        assert_eq!(relative, "subfolder/file.txt");
    }

    #[test]
    fn test_get_relative_path_same_path() {
        use super::get_relative_path;
        
        let base = Path::new("/home/user/documents");
        let file = Path::new("/home/user/documents");
        
        let relative = get_relative_path(base, file);
        assert_eq!(relative, "");
    }

    #[test]
    fn test_get_relative_path_not_prefix() {
        use super::get_relative_path;
        
        let base = Path::new("/home/user/documents");
        let file = Path::new("/other/path/file.txt");
        
        let relative = get_relative_path(base, file);
        
        // Should return the full path when not a prefix
        #[cfg(windows)]
        assert!(relative.contains("file.txt"));
        
        #[cfg(not(windows))]
        assert_eq!(relative, "/other/path/file.txt");
    }

    #[test]
    fn test_file_hash_structure() {
        let hash = FileHash {
            path: PathBuf::from("/test/file.txt"),
            relative_path: "file.txt".to_string(),
            hash: "abc123".to_string(),
            size: 1024,
        };
        
        assert_eq!(hash.relative_path, "file.txt");
        assert_eq!(hash.hash, "abc123");
        assert_eq!(hash.size, 1024);
    }

    #[test]
    fn test_hash_directory_cancellation() {
        let temp_dir = create_temp_dir("hash_cancel");
        
        // Create several test files
        for i in 0..10 {
            fs::write(temp_dir.join(format!("file{}.txt", i)), format!("content{}", i)).unwrap();
        }
        
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel_flag);
        
        let handle = hash_directory(&temp_dir, tx, cancel_clone);
        
        // Cancel after receiving first progress update
        let _= rx.recv_timeout(std::time::Duration::from_secs(1));
        cancel_flag.store(true, Ordering::Relaxed);
        
        // Wait for completion
        let result = handle.join().expect("Hash thread panicked");
        
        // Should be cancelled        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.contains("Cancelled"));
        }
        
        cleanup_temp_dir(&temp_dir);
    }
}