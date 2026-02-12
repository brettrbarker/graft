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
}

impl CommandHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 100,
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
        let entry2 = create_test_entry("C:\\Second", "D:\\Second");
        let entry1_id = entry1.id;
        
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
}
