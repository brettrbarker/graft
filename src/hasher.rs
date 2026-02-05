//! File hashing module for verifying file transfers

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use walkdir::WalkDir;

/// Result of hashing a single file
#[derive(Clone, Debug)]
pub struct FileHash {
    pub path: PathBuf,
    pub relative_path: String,
    pub hash: String,
    pub size: u64,
}

/// Result of comparing source and destination hashes
#[derive(Clone, Debug)]
pub struct HashVerification {
    pub matched: Vec<(String, String)>,       // (relative_path, hash)
    pub mismatched: Vec<(String, String, String)>, // (relative_path, source_hash, dest_hash)
    pub missing_in_dest: Vec<String>,
    pub extra_in_dest: Vec<String>,
}

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
) -> thread::JoinHandle<Result<Vec<FileHash>, String>> {
    let dir = dir.to_path_buf();
    
    thread::spawn(move || {
        let files = collect_files(&dir)?;
        let total = files.len();
        
        let _ = progress_tx.send(HashProgress::Starting(total));
        
        let mut hashes = Vec::new();
        
        for file_path in files {
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
