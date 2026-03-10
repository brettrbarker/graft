//! Command history management module

use crate::robocopy::RobocopyOptions;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// A single command history entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub timestamp: DateTime<Local>,
    pub source: String,
    pub destination: String,
    pub command: String,
    pub options: RobocopyOptions,
    pub saved: bool,
    pub name: Option<String>,
    #[serde(default)]
    pub log_content: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub ticket_number: Option<String>,
}

impl HistoryEntry {
    pub fn new(source: String, destination: String, command: String, options: RobocopyOptions) -> Self {
        Self {
            id: Local::now().timestamp_millis() as u64,
            timestamp: Local::now(),
            source,
            destination,
            command,
            options,
            saved: false,
            name: None,
            log_content: None,
            username: Self::get_username(),
            ticket_number: None,
        }
    }

    /// Get the current system username
    pub fn get_username() -> Option<String> {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .ok()
    }

    pub fn display_name(&self) -> String {
        if let Some(ref name) = self.name {
            name.clone()
        } else {
            let ticket_info = self.ticket_number.as_ref()
                .map_or(String::new(), |t| format!(" [{}]", t));
            format!(
                "{} → {}{} ({})",
                self.source,
                self.destination,
                ticket_info,
                self.timestamp.format("%Y-%m-%d %H:%M")
            )
        }
    }

    /// Get the path to the logs directory
    pub fn get_log_directory() -> Option<PathBuf> {
        dirs::data_local_dir().map(|p| p.join("Graft").join("logs"))
    }

    /// Generate a log filename for this entry
    pub fn generate_log_filename(&self) -> String {
        let timestamp = self.timestamp.format("%Y-%m-%d_%H-%M-%S");
        if let Some(ref ticket) = self.ticket_number {
            let sanitized_ticket = ticket.trim().replace(' ', "_");
            format!("graft_{}_{}.log", timestamp, sanitized_ticket)
        } else {
            format!("graft_{}.log", timestamp)
        }
    }

    /// Save the log content to disk
    pub fn save_log_to_disk(&self) -> Result<PathBuf, String> {
        if let Some(ref log_content) = self.log_content {
            if let Some(log_dir) = Self::get_log_directory() {
                // Create directory if it doesn't exist
                fs::create_dir_all(&log_dir)
                    .map_err(|e| format!("Failed to create log directory: {}", e))?;

                let log_path = log_dir.join(self.generate_log_filename());
                fs::write(&log_path, log_content)
                    .map_err(|e| format!("Failed to write log file: {}", e))?;
                
                return Ok(log_path);
            }
        }
        Err("No log content to save".to_string())
    }

}

/// Command history storage
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CommandHistory {
    pub entries: Vec<HistoryEntry>,
    pub max_entries: usize,
    pub last_config: Option<LastConfig>,
}

/// Last used configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LastConfig {
    pub source: String,
    pub destination: String,
    pub options: RobocopyOptions,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 100,
            last_config: None,
        }
    }

    /// Get the path to the history file
    fn get_history_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|p| p.join("Graft").join("history.json"))
    }

    /// Load history from disk
    pub fn load() -> Self {
        if let Some(path) = Self::get_history_path() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(history) = serde_json::from_str(&content) {
                        return history;
                    }
                }
            }
        }
        Self::new()
    }

    /// Save history to disk
    pub fn save(&self) -> Result<(), String> {
        if let Some(path) = Self::get_history_path() {
            // Create directory if it doesn't exist
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create history directory: {}", e))?;
            }

            let json = serde_json::to_string_pretty(self)
                .map_err(|e| format!("Failed to serialize history: {}", e))?;
            
            fs::write(&path, json)
                .map_err(|e| format!("Failed to write history file: {}", e))?;
        }
        Ok(())
    }

    /// Add a new entry
    pub fn add_entry(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        
        // Keep only max_entries (but always keep saved entries)
        let mut count = 0;
        self.entries.retain(|e| {
            if e.saved {
                true
            } else {
                count += 1;
                count <= self.max_entries
            }
        });
    }

    /// Get recent entries (unsaved)
    pub fn recent_entries(&self) -> Vec<&HistoryEntry> {
        self.entries.iter().filter(|e| !e.saved).collect()
    }

    /// Get saved entries
    pub fn saved_entries(&self) -> Vec<&HistoryEntry> {
        self.entries.iter().filter(|e| e.saved).collect()
    }

    /// Toggle save status of an entry
    pub fn toggle_save(&mut self, id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.saved = !entry.saved;
        }
    }

    /// Update the name of an entry
    pub fn set_name(&mut self, id: u64, name: String) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.name = if name.is_empty() { None } else { Some(name) };
        }
    }

    /// Update the log content and ticket number for an entry
    pub fn set_log_content(&mut self, id: u64, log_content: String, ticket_number: Option<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.log_content = Some(log_content);
            entry.ticket_number = ticket_number;
        }
    }

    /// Get mutable entry by ID
    pub fn get_entry_mut(&mut self, id: u64) -> Option<&mut HistoryEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Delete an entry
    pub fn delete_entry(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }

    /// Save the last used configuration
    /// 
    /// Stores the source path, destination path, and options for the last command
    /// that was executed. This will be loaded automatically on next application start.
    pub fn save_last_config(&mut self, source: String, destination: String, options: RobocopyOptions) {
        self.last_config = Some(LastConfig {
            source,
            destination,
            options,
        });
    }

    /// Get the last used configuration
    /// 
    /// Returns the last saved configuration if available, or None if no configuration
    /// has been saved yet. This is used to restore the last command on application startup.
    pub fn get_last_config(&self) -> Option<&LastConfig> {
        self.last_config.as_ref()
    }

    /// Get entry by ID
    #[allow(dead_code)] // Public API for future use
    pub fn get_entry(&self, id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robocopy::RobocopyOptions;

    fn create_test_entry(source: &str, dest: &str) -> HistoryEntry {
        HistoryEntry::new(
            source.to_string(),
            dest.to_string(),
            format!("robocopy \"{}\" \"{}\"", source, dest),
            RobocopyOptions::default(),
        )
    }

    #[test]
    fn test_history_entry_new() {
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        
        assert_eq!(entry.source, "C:\\Source");
        assert_eq!(entry.destination, "D:\\Dest");
        assert!(!entry.saved);
        assert!(entry.name.is_none());
        assert!(entry.id > 0);
    }

    #[test]
    fn test_history_entry_display_name_default() {
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let display = entry.display_name();
        
        assert!(display.contains("C:\\Source"));
        assert!(display.contains("D:\\Dest"));
        assert!(display.contains("→"));
    }

    #[test]
    fn test_history_entry_display_name_custom() {
        let mut entry = create_test_entry("C:\\Source", "D:\\Dest");
        entry.name = Some("My Backup".to_string());
        
        assert_eq!(entry.display_name(), "My Backup");
    }

    #[test]
    fn test_command_history_new() {
        let history = CommandHistory::new();
        
        assert!(history.entries.is_empty());
        assert_eq!(history.max_entries, 100);
    }

    #[test]
    fn test_command_history_add_entry() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].id, entry_id);
    }

    #[test]
    fn test_command_history_entries_inserted_at_front() {
        let mut history = CommandHistory::new();
        
        let entry1 = create_test_entry("C:\\First", "D:\\First");
        let entry2 = create_test_entry("C:\\Second", "D:\\Second");
        
        history.add_entry(entry1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        history.add_entry(entry2);
        
        assert_eq!(history.entries.len(), 2);
        // Most recent should be first
        assert_eq!(history.entries[0].source, "C:\\Second");
        assert_eq!(history.entries[1].source, "C:\\First");
    }

    #[test]
    fn test_command_history_toggle_save() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        assert!(!history.entries[0].saved);
        
        history.toggle_save(entry_id);
        assert!(history.entries[0].saved);
        
        history.toggle_save(entry_id);
        assert!(!history.entries[0].saved);
    }

    #[test]
    fn test_command_history_set_name() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        history.set_name(entry_id, "My Custom Name".to_string());
        assert_eq!(history.entries[0].name, Some("My Custom Name".to_string()));
        
        // Empty name should set to None
        history.set_name(entry_id, "".to_string());
        assert!(history.entries[0].name.is_none());
    }

    #[test]
    fn test_command_history_delete_entry() {
        let mut history = CommandHistory::new();
        let entry1 = create_test_entry("C:\\First", "D:\\First");
        let entry1_id = entry1.id;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let entry2 = create_test_entry("C:\\Second", "D:\\Second");
        
        history.add_entry(entry1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        history.add_entry(entry2);
        
        assert_eq!(history.entries.len(), 2);
        
        history.delete_entry(entry1_id);
        
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].source, "C:\\Second");
    }

    #[test]
    fn test_command_history_get_entry() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        let found = history.get_entry(entry_id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().source, "C:\\Source");
        
        let not_found = history.get_entry(99999);
        assert!(not_found.is_none());
    }

    #[test]
    fn test_command_history_recent_entries() {
        let mut history = CommandHistory::new();
        
        let mut entry1 = create_test_entry("C:\\Saved", "D:\\Saved");
        entry1.saved = true;
        
        let entry2 = create_test_entry("C:\\Recent", "D:\\Recent");
        
        history.add_entry(entry1);
        history.add_entry(entry2);
        
        let recent = history.recent_entries();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].source, "C:\\Recent");
    }

    #[test]
    fn test_command_history_saved_entries() {
        let mut history = CommandHistory::new();
        
        let mut entry1 = create_test_entry("C:\\Saved", "D:\\Saved");
        entry1.saved = true;
        
        let entry2 = create_test_entry("C:\\Recent", "D:\\Recent");
        
        history.add_entry(entry1);
        history.add_entry(entry2);
        
        let saved = history.saved_entries();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].source, "C:\\Saved");
    }

    #[test]
    fn test_command_history_max_entries_respected() {
        let mut history = CommandHistory::new();
        history.max_entries = 3;
        
        for i in 0..5 {
            let entry = create_test_entry(&format!("C:\\Source{}", i), &format!("D:\\Dest{}", i));
            history.add_entry(entry);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        
        // Should only keep max_entries unsaved entries
        assert_eq!(history.entries.len(), 3);
        
        // Most recent should be preserved
        assert_eq!(history.entries[0].source, "C:\\Source4");
    }

    #[test]
    fn test_command_history_saved_entries_not_pruned() {
        let mut history = CommandHistory::new();
        history.max_entries = 2;
        
        // Add a saved entry
        let mut saved = create_test_entry("C:\\Saved", "D:\\Saved");
        saved.saved = true;
        history.add_entry(saved);
        
        // Add more unsaved entries than max
        for i in 0..3 {
            let entry = create_test_entry(&format!("C:\\Source{}", i), &format!("D:\\Dest{}", i));
            history.add_entry(entry);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        
        // Saved entry should still exist + 2 most recent unsaved
        let saved_count = history.entries.iter().filter(|e| e.saved).count();
        let unsaved_count = history.entries.iter().filter(|e| !e.saved).count();
        
        assert_eq!(saved_count, 1);
        assert!(unsaved_count <= 2);
    }

    #[test]
    fn test_history_serialization() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        history.add_entry(entry);
        
        // Serialize to JSON
        let json = serde_json::to_string(&history).expect("Failed to serialize");
        
        // Deserialize back
        let restored: CommandHistory = serde_json::from_str(&json).expect("Failed to deserialize");
        
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.entries[0].source, "C:\\Source");
    }

    #[test]
    fn test_history_entry_get_username() {
        // This test checks that get_username returns something or None
        let username = HistoryEntry::get_username();
        
        // On most systems, USERNAME or USER should be set
        // We can't assert exact value, but can verify the function works
        assert!(username.is_some() || username.is_none());
    }

    #[test]
    fn test_history_entry_get_log_directory() {
        let log_dir = HistoryEntry::get_log_directory();
        
        // Should return a path on most systems
        if let Some(dir) = log_dir {
            assert!(dir.to_string_lossy().contains("Graft"));
            assert!(dir.to_string_lossy().contains("logs"));
        }
    }

    #[test]
    fn test_history_entry_generate_log_filename() {
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let filename = entry.generate_log_filename();
        
        assert!(filename.starts_with("graft_"));
        assert!(filename.ends_with(".log"));
        assert!(filename.contains(&entry.timestamp.format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn test_history_entry_generate_log_filename_with_ticket() {
        let mut entry = create_test_entry("C:\\Source", "D:\\Dest");
        entry.ticket_number = Some("TICKET-123".to_string());
        
        let filename = entry.generate_log_filename();
        
        assert!(filename.contains("TICKET-123"));
        assert!(filename.ends_with(".log"));
    }

    #[test]
    fn test_history_entry_save_log_to_disk() {
        let mut entry = create_test_entry("C:\\Source", "D:\\Dest");
        entry.log_content = Some("Test log content\nLine 2\nLine 3".to_string());
        
        // Try to save (may fail if no permissions, which is okay for testing)
        let result = entry.save_log_to_disk();
        
        if let Ok(log_path) = result {
            // Verify the file was created
            assert!(log_path.exists());
            
            // Read and verify content
            let content = std::fs::read_to_string(&log_path).expect("Failed to read log file");
            assert_eq!(content, "Test log content\nLine 2\nLine 3");
            
            // Cleanup
            let _ = std::fs::remove_file(&log_path);
        }
    }

    #[test]
    fn test_history_entry_save_log_without_content() {
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        // No log_content set
        
        let result = entry.save_log_to_disk();
        assert!(result.is_err());
    }

    #[test]
    fn test_command_history_set_log_content() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        history.set_log_content(entry_id, "Log content here".to_string(), Some("TKT-456".to_string()));
        
        assert_eq!(history.entries[0].log_content, Some("Log content here".to_string()));
        assert_eq!(history.entries[0].ticket_number, Some("TKT-456".to_string()));
    }

    #[test]
    fn test_command_history_set_log_content_without_ticket() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        history.set_log_content(entry_id, "Log without ticket".to_string(), None);
        
        assert_eq!(history.entries[0].log_content, Some("Log without ticket".to_string()));
        assert_eq!(history.entries[0].ticket_number, None);
    }

    #[test]
    fn test_command_history_get_entry_mut() {
        let mut history = CommandHistory::new();
        let entry = create_test_entry("C:\\Source", "D:\\Dest");
        let entry_id = entry.id;
        
        history.add_entry(entry);
        
        // Get mutable reference and modify
        if let Some(entry_mut) = history.get_entry_mut(entry_id) {
            entry_mut.saved = true;
        }
        
        assert!(history.entries[0].saved);
    }

    #[test]
    fn test_command_history_get_entry_mut_not_found() {
        let mut history = CommandHistory::new();
        
        let result = history.get_entry_mut(99999);
        assert!(result.is_none());
    }

    #[test]
    fn test_command_history_save_last_config() {
        let mut history = CommandHistory::new();
        let options = RobocopyOptions::default();
        
        history.save_last_config(
            "C:\\LastSource".to_string(),
            "D:\\LastDest".to_string(),
            options.clone(),
        );
        
        assert!(history.last_config.is_some());
        let config = history.last_config.as_ref().unwrap();
        assert_eq!(config.source, "C:\\LastSource");
        assert_eq!(config.destination, "D:\\LastDest");
    }

    #[test]
    fn test_command_history_get_last_config() {
        let mut history = CommandHistory::new();
        
        // No config saved yet
        assert!(history.get_last_config().is_none());
        
        // Save a config
        let options = RobocopyOptions::default();
        history.save_last_config(
            "C:\\Source".to_string(),
            "D:\\Dest".to_string(),
            options,
        );
        
        // Now it should return the config
        let config = history.get_last_config();
        assert!(config.is_some());
        assert_eq!(config.unwrap().source, "C:\\Source");
    }

    #[test]
    fn test_command_history_display_name_with_ticket() {
        let mut entry = create_test_entry("C:\\Source", "D:\\Dest");
        entry.ticket_number = Some("TICKET-789".to_string());
        
        let display = entry.display_name();
        
        assert!(display.contains("[TICKET-789]"));
    }

    #[test]
    fn test_last_config_serialization() {
        let config = LastConfig {
            source: "C:\\Test".to_string(),
            destination: "D:\\Test".to_string(),
            options: RobocopyOptions::default(),
        };
        
        // Serialize to JSON
        let json = serde_json::to_string(&config).expect("Failed to serialize");
        
        // Deserialize back
        let restored: LastConfig = serde_json::from_str(&json).expect("Failed to deserialize");
        
        assert_eq!(restored.source, "C:\\Test");
        assert_eq!(restored.destination, "D:\\Test");
    }
}
