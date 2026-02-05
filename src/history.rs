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
        }
    }

    pub fn display_name(&self) -> String {
        if let Some(ref name) = self.name {
            name.clone()
        } else {
            format!(
                "{} → {} ({})",
                self.source,
                self.destination,
                self.timestamp.format("%Y-%m-%d %H:%M")
            )
        }
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
        dirs::data_local_dir().map(|p| p.join("RoboAft").join("history.json"))
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

    /// Delete an entry
    pub fn delete_entry(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }

    /// Get entry by ID
    pub fn get_entry(&self, id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}
