//! Main application GUI module

use crate::hasher::{
    format_hash_results, hash_directory, FileHash,
    HashProgress,
};
use crate::history::{CommandHistory, HistoryEntry};
use crate::robocopy::{PresetGroup, RobocopyOption, RobocopyOptions};
use chrono::Local;
use eframe::egui;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

/// Application state for running operations
#[derive(PartialEq)]
enum AppState {
    Idle,
    Running,
    Hashing,
}

/// Tab selection for the main view
#[derive(PartialEq, Clone, Copy)]
enum MainTab {
    Options,
    History,
}

/// The main application struct
/// Transfer statistics parsed from robocopy output
#[derive(Default, Clone, Debug)]
struct TransferStats {
    files_total: u64,
    files_copied: u64,
    files_skipped: u64,
    files_mismatch: u64,
    files_failed: u64,
    files_extras: u64,
    dirs_total: u64,
    dirs_copied: u64,
    dirs_skipped: u64,
    dirs_failed: u64,
    dirs_extras: u64,
    bytes_total: String,
    bytes_copied: String,
    bytes_failed: String,
    speed: String,
    robocopy_exit_code: i32,
}

pub struct GraftApp {
    // Paths
    source_path: String,
    destination_path: String,

    // Robocopy options
    options: RobocopyOptions,

    // History
    history: CommandHistory,
    selected_history_id: Option<u64>,
    rename_buffer: String,
    current_entry_id: Option<u64>,

    // Console output
    console_output: Vec<String>,
    console_scroll_to_bottom: bool,

    // Log
    log_entries: Vec<String>,

    // State
    state: AppState,
    current_tab: MainTab,
    show_destructive_warning: bool,
    destructive_warning_text: String,
    show_about: bool,

    // Hashing
    enable_hashing: bool,
    source_hashes: Vec<FileHash>,
    hash_progress_text: String,
    hash_files_processed: usize,
    hash_files_total: usize,

    // AFT Ticket
    aft_ticket_number: String,

    // Transfer statistics
    transfer_stats: TransferStats,

    // Channels for async operations
    console_rx: Option<Receiver<String>>,
    hash_progress_rx: Option<Receiver<HashProgress>>,
    
    // Child process handle
    robocopy_child: Option<Child>,
    output_thread: Option<JoinHandle<()>>,
    hash_thread_source: Option<JoinHandle<Result<Vec<FileHash>, String>>>,
}

impl GraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Apply Material Design inspired dark theme
        Self::setup_custom_theme(&cc.egui_ctx);
        
        let history = CommandHistory::load();
        
        // Load last config if available
        let (source_path, destination_path, options) = if let Some(last_config) = history.get_last_config() {
            (
                last_config.source.clone(),
                last_config.destination.clone(),
                last_config.options.clone(),
            )
        } else {
            (String::new(), String::new(), RobocopyOptions::default())
        };
        
        Self {
            source_path,
            destination_path,
            options,
            history,
            selected_history_id: None,
            rename_buffer: String::new(),
            current_entry_id: None,
            console_output: Vec::new(),
            console_scroll_to_bottom: false,
            log_entries: Vec::new(),
            state: AppState::Idle,
            current_tab: MainTab::Options,
            show_destructive_warning: false,
            destructive_warning_text: String::new(),
            show_about: false,
            enable_hashing: true,
            source_hashes: Vec::new(),
            hash_progress_text: String::new(),
            hash_files_processed: 0,
            hash_files_total: 0,
            aft_ticket_number: String::new(),
            transfer_stats: TransferStats::default(),
            console_rx: None,
            hash_progress_rx: None,
            robocopy_child: None,
            output_thread: None,
            hash_thread_source: None,
        }
    }

    /// Setup Material Design inspired dark theme
    fn setup_custom_theme(ctx: &egui::Context) {
        // Material Design 3 Dark Theme colors
        // Surface colors
        let surface = egui::Color32::from_rgb(28, 27, 31);           // Dark surface
        let surface_container = egui::Color32::from_rgb(33, 31, 38); // Slightly lighter
        let surface_container_high = egui::Color32::from_rgb(43, 41, 48);
        let surface_container_highest = egui::Color32::from_rgb(54, 52, 59);
        
        // Primary colors (Teal/Cyan accent - Material inspired)
        let primary = egui::Color32::from_rgb(79, 195, 247);         // Light blue accent
        let primary_container = egui::Color32::from_rgb(0, 77, 100); // Dark teal
        
        // Secondary colors  
        let secondary = egui::Color32::from_rgb(128, 203, 196);      // Teal
        
        // Text colors
        let on_surface = egui::Color32::from_rgb(230, 225, 229);     // Primary text
        let on_surface_variant = egui::Color32::from_rgb(202, 196, 208); // Secondary text
        
        // Accent/status colors
        let error = egui::Color32::from_rgb(255, 84, 73);
        
        let mut style = (*ctx.style()).clone();
        
        // Visuals for dark mode
        let mut visuals = egui::Visuals::dark();
        
        // Window and panel backgrounds
        visuals.window_fill = surface;
        visuals.panel_fill = surface;
        visuals.extreme_bg_color = surface_container;
        visuals.faint_bg_color = surface_container_high;
        
        // Widget backgrounds
        visuals.widgets.noninteractive.bg_fill = surface_container;
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, on_surface_variant);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, surface_container_highest);
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
        
        visuals.widgets.inactive.bg_fill = surface_container_high;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, on_surface);
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, surface_container_highest);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
        
        visuals.widgets.hovered.bg_fill = surface_container_highest;
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, primary);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, primary);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
        
        visuals.widgets.active.bg_fill = primary_container;
        visuals.widgets.active.fg_stroke = egui::Stroke::new(2.0, primary);
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, primary);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
        
        visuals.widgets.open.bg_fill = surface_container_high;
        visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, on_surface);
        visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, primary);
        visuals.widgets.open.corner_radius = egui::CornerRadius::same(8);
        
        // Selection colors
        visuals.selection.bg_fill = primary_container;
        visuals.selection.stroke = egui::Stroke::new(1.0, primary);
        
        // Hyperlink color
        visuals.hyperlink_color = secondary;
        
        // Error/warning colors
        visuals.error_fg_color = error;
        visuals.warn_fg_color = egui::Color32::from_rgb(255, 183, 77); // Orange warning
        
        // Window styling
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.window_shadow = egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: egui::Color32::from_black_alpha(80),
        };
        visuals.window_stroke = egui::Stroke::new(1.0, surface_container_highest);
        
        // Popup styling
        visuals.popup_shadow = egui::Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: egui::Color32::from_black_alpha(60),
        };
        
        // Separator/stripe colors
        visuals.striped = true;
        
        // Apply visuals
        style.visuals = visuals;
        
        // Spacing and sizing - Material Design likes generous spacing
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
        style.spacing.button_padding = egui::vec2(16.0, 8.0);
        style.spacing.indent = 24.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.bar_outer_margin = 4.0;
        
        // Text styles
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        );
        
        ctx.set_style(style);
    }

    fn log(&mut self, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        self.log_entries
            .push(format!("[{}] {}", timestamp, message));
    }

    fn add_console_line(&mut self, line: String) {
        self.console_output.push(line);
        self.console_scroll_to_bottom = true;
        // Keep console buffer reasonable
        if self.console_output.len() > 10000 {
            self.console_output.drain(0..5000);
        }
    }

    fn clear_console(&mut self) {
        self.console_output.clear();
    }

    fn destructive_option_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.options.mirror.enabled {
            labels.push("Mirror (/MIR)");
        }
        if self.options.purge.enabled {
            labels.push("Purge (/PURGE)");
        }
        if self.options.move_files.enabled {
            labels.push("Move files (/MOV)");
        }
        if self.options.move_files_dirs.enabled {
            labels.push("Move files and dirs (/MOVE)");
        }
        labels
    }

    fn request_start_robocopy(&mut self) {
        let destructive = self.destructive_option_labels();
        if destructive.is_empty() {
            self.start_robocopy();
            return;
        }

        let mut warning = String::from(
            "Destructive options are enabled. These options can delete or move files from the destination or source.\n\nEnabled:\n",
        );
        for label in destructive {
            warning.push_str("- ");
            warning.push_str(label);
            warning.push('\n');
        }
        warning.push_str("\nReview your source/destination paths before continuing.");
        self.destructive_warning_text = warning;
        self.show_destructive_warning = true;
    }

    fn start_robocopy(&mut self) {
        if self.source_path.is_empty() || self.destination_path.is_empty() {
            self.log("Error: Source and destination paths are required");
            return;
        }

        let command = self.options.build_command_string(&self.source_path, &self.destination_path);
        self.log_entries.clear(); // Clear log for new transfer
        self.log(&format!("Preset: {}", self.options.current_preset.name()));
        self.log(&format!("Starting: {}", command));
        self.clear_console();
        self.add_console_line(format!(">>> {}", command));
        self.add_console_line(String::new());

        // Save to history
        let mut entry = HistoryEntry::new(
            self.source_path.clone(),
            self.destination_path.clone(),
            command.clone(),
            self.options.clone(),
        );
        // Store ticket number if present
        if !self.aft_ticket_number.is_empty() {
            entry.ticket_number = Some(self.aft_ticket_number.clone());
        }
        let entry_id = entry.id;
        self.history.add_entry(entry);
        
        // Save last config for next session
        self.history.save_last_config(
            self.source_path.clone(),
            self.destination_path.clone(),
            self.options.clone(),
        );
        let _ = self.history.save();
        
        // Track current entry for log saving
        self.current_entry_id = Some(entry_id);

        // Build args
        let args = self.options.build_args(&self.source_path, &self.destination_path);

        // Start robocopy process
        match Command::new("robocopy")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                // Create channel for console output
                let (tx, rx) = mpsc::channel::<String>();
                self.console_rx = Some(rx);

                // Spawn thread to read output
                let tx_clone = tx.clone();
                let handle = thread::spawn(move || {
                    if let Some(stdout) = stdout {
                        let reader = BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            if tx_clone.send(line).is_err() {
                                break;
                            }
                        }
                    }
                    if let Some(stderr) = stderr {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            if tx_clone.send(format!("[ERROR] {}", line)).is_err() {
                                break;
                            }
                        }
                    }
                    let _ = tx_clone.send("__DONE__".to_string());
                });

                self.output_thread = Some(handle);
                self.robocopy_child = Some(child);
                self.state = AppState::Running;
            }
            Err(e) => {
                self.log(&format!("Failed to start robocopy: {}", e));
                self.add_console_line(format!("Error: Failed to start robocopy: {}", e));
            }
        }
    }

    /// Parse robocopy summary statistics from console output.
    /// Robocopy prints a summary table at the end like:
    ///                Total    Copied   Skipped  Mismatch    FAILED    Extras
    ///    Dirs :         5         3         2         0         0         0
    ///   Files :        10         8         2         0         0         0
    ///   Bytes :   1.234 m   1.001 m   233.0 k         0         0         0
    ///   Speed :           1234567 Bytes/sec.
    fn parse_robocopy_stats(&mut self) {
        self.transfer_stats = TransferStats::default();

        // Search console output from the end for the summary lines
        for line in self.console_output.iter().rev().take(30) {
            let trimmed = line.trim();
            if trimmed.starts_with("Dirs :") || trimmed.starts_with("Dirs:") {
                let nums = Self::parse_stat_line(trimmed);
                if nums.len() >= 6 {
                    self.transfer_stats.dirs_total = nums[0];
                    self.transfer_stats.dirs_copied = nums[1];
                    self.transfer_stats.dirs_skipped = nums[2];
                    // index 3 = mismatch (not tracked for dirs)
                    self.transfer_stats.dirs_failed = nums[4];
                    self.transfer_stats.dirs_extras = nums[5];
                }
            } else if trimmed.starts_with("Files :") || trimmed.starts_with("Files:") {
                let nums = Self::parse_stat_line(trimmed);
                if nums.len() >= 6 {
                    self.transfer_stats.files_total = nums[0];
                    self.transfer_stats.files_copied = nums[1];
                    self.transfer_stats.files_skipped = nums[2];
                    self.transfer_stats.files_mismatch = nums[3];
                    self.transfer_stats.files_failed = nums[4];
                    self.transfer_stats.files_extras = nums[5];
                }
            } else if trimmed.starts_with("Bytes :") || trimmed.starts_with("Bytes:") {
                // Bytes line may have human-readable values like "1.234 m"
                let after_colon = trimmed.splitn(2, ':').nth(1).unwrap_or("").trim();
                let parts: Vec<&str> = after_colon.split_whitespace().collect();
                // Parse byte groups: each may be a number or "number unit"
                self.transfer_stats.bytes_total = Self::extract_byte_value(&parts, 0);
                self.transfer_stats.bytes_copied = Self::extract_byte_value(&parts, 1);
                self.transfer_stats.bytes_failed = Self::extract_byte_value(&parts, 4);
            } else if trimmed.starts_with("Speed :") || trimmed.starts_with("Speed:") {
                let after_colon = trimmed.splitn(2, ':').nth(1).unwrap_or("").trim();
                self.transfer_stats.speed = after_colon.to_string();
            }
        }
    }

    /// Parse numeric values from a robocopy stat line (after the colon)
    fn parse_stat_line(line: &str) -> Vec<u64> {
        let after_colon = line.splitn(2, ':').nth(1).unwrap_or("");
        after_colon
            .split_whitespace()
            .filter_map(|s| s.replace(',', "").parse::<u64>().ok())
            .collect()
    }

    /// Extract a human-readable byte value from robocopy output parts.
    /// Values can be plain numbers or "1.234 m", "500 k", etc.
    fn extract_byte_value(parts: &[&str], index: usize) -> String {
        // Robocopy byte values: either a plain number or a number followed by a size suffix
        // Since the split is whitespace-based, we try to find the Nth numeric token
        // and check if the next token is a size suffix
        let mut count = 0usize;
        let mut i = 0;
        while i < parts.len() {
            // Check if this token looks like a number (starts with digit or has decimal)
            let is_num = parts[i].replace(',', "").parse::<f64>().is_ok();
            if is_num {
                if count == index {
                    // Check if next token is a size suffix
                    if i + 1 < parts.len() {
                        let next = parts[i + 1].to_lowercase();
                        if ["k", "m", "g", "t"].contains(&next.as_str()) {
                            return format!("{} {}", parts[i], parts[i + 1]);
                        }
                    }
                    return parts[i].to_string();
                }
                count += 1;
                // Skip size suffix if present
                if i + 1 < parts.len() {
                    let next = parts[i + 1].to_lowercase();
                    if ["k", "m", "g", "t"].contains(&next.as_str()) {
                        i += 1;
                    }
                }
            }
            i += 1;
        }
        String::new()
    }

    fn check_robocopy_done(&mut self) {
        if self.state != AppState::Running {
            return;
        }

        // Collect all messages first to avoid borrow issues
        let messages: Vec<String> = if let Some(ref rx) = self.console_rx {
            let mut msgs = Vec::new();
            while let Ok(line) = rx.try_recv() {
                msgs.push(line);
            }
            msgs
        } else {
            Vec::new()
        };

        // Process collected messages
        for line in messages {
            if line == "__DONE__" {
                // Process finished
                if let Some(mut child) = self.robocopy_child.take() {
                    match child.wait() {
                        Ok(status) => {
                            let exit_code = status.code().unwrap_or(-1);
                            self.transfer_stats.robocopy_exit_code = exit_code;
                            self.add_console_line(String::new());
                            self.add_console_line(format!(
                                ">>> Robocopy finished with exit code: {}",
                                exit_code
                            ));
                            self.log(&format!("Robocopy completed with exit code: {}", exit_code));

                            // Robocopy exit codes:
                            // 0 = No files copied
                            // 1 = Files copied successfully
                            // 2 = Extra files detected
                            // 4 = Mismatched files
                            // 8+ = Errors occurred
                            let message = match exit_code {
                                0 => "No files were copied. Source and destination are in sync.",
                                1 => "All files were copied successfully.",
                                2 => "Extra files or directories detected.",
                                3 => "Files copied and extra files detected.",
                                4 => "Some mismatched files or directories detected.",
                                5..=7 => "Files copied with some issues.",
                                8..=15 => "Some files or directories could not be copied (copy errors).",
                                16.. => "Serious error. No files were copied.",
                                _ => "Unknown exit code.",
                            };
                            self.add_console_line(format!(">>> {}", message));
                            self.log(message);

                            // Parse robocopy summary statistics from console output
                            self.parse_robocopy_stats();
                            self.log(&format!("Files - Total: {}, Copied: {}, Skipped: {}, Failed: {}, Extras: {}",
                                self.transfer_stats.files_total,
                                self.transfer_stats.files_copied,
                                self.transfer_stats.files_skipped,
                                self.transfer_stats.files_failed,
                                self.transfer_stats.files_extras,
                            ));
                            self.log(&format!("Dirs  - Total: {}, Copied: {}, Skipped: {}, Failed: {}, Extras: {}",
                                self.transfer_stats.dirs_total,
                                self.transfer_stats.dirs_copied,
                                self.transfer_stats.dirs_skipped,
                                self.transfer_stats.dirs_failed,
                                self.transfer_stats.dirs_extras,
                            ));
                        }
                        Err(e) => {
                            self.log(&format!("Error waiting for process: {}", e));
                        }
                    }
                }

                self.console_rx = None;
                self.output_thread = None;

                // Start hashing if enabled and robocopy succeeded (exit code < 8)
                let last_exit_code = self.transfer_stats.robocopy_exit_code;
                
                if self.enable_hashing && last_exit_code < 8 {
                    self.start_hashing();
                } else if self.enable_hashing && last_exit_code >= 8 {
                    self.log("Skipping source file hashing due to robocopy errors");
                    self.add_console_line(">>> Skipping source file hashing due to robocopy errors".to_string());
                    self.save_log_to_history();
                    self.state = AppState::Idle;
                } else {
                    self.save_log_to_history();
                    self.state = AppState::Idle;
                }
                return;
            }
            self.add_console_line(line);
        }
    }

    fn start_hashing(&mut self) {
        self.log("Starting source file hashing...");
        self.add_console_line(String::new());
        self.add_console_line(">>> Starting source file hashing...".to_string());

        self.source_hashes.clear();
        self.hash_progress_text = "Initializing...".to_string();
        self.hash_files_processed = 0;
        self.hash_files_total = 0;

        let (tx, rx) = mpsc::channel::<HashProgress>();
        self.hash_progress_rx = Some(rx);

        // Start hashing source only
        let source_path = PathBuf::from(&self.source_path);
        self.hash_thread_source = Some(hash_directory(&source_path, tx));

        self.state = AppState::Hashing;
    }

    fn check_hashing_done(&mut self) {
        if self.state != AppState::Hashing {
            return;
        }

        // Collect all progress updates first to avoid borrow issues
        let updates: Vec<HashProgress> = if let Some(ref rx) = self.hash_progress_rx {
            let mut upds = Vec::new();
            while let Ok(progress) = rx.try_recv() {
                upds.push(progress);
            }
            upds
        } else {
            Vec::new()
        };

        // Process collected updates
        for progress in updates {
            match progress {
                HashProgress::Starting(total) => {
                    self.hash_files_total += total;
                    self.hash_progress_text = format!("Hashing {} source files...", self.hash_files_total);
                }
                HashProgress::FileStarted(path) => {
                    self.hash_progress_text = format!("Hashing: {}", path);
                }
                HashProgress::FileComplete(file_hash) => {
                    self.hash_files_processed += 1;
                    self.add_console_line(format!(
                        "  Hashed: {} ({} bytes)",
                        file_hash.relative_path,
                        file_hash.size
                    ));
                }
                HashProgress::Complete(hashes) => {
                    self.source_hashes = hashes;
                    self.add_console_line(format!(
                        ">>> Source hashing complete: {} files",
                        self.source_hashes.len()
                    ));
                }
                HashProgress::Error(e) => {
                    self.log(&format!("Hash error: {}", e));
                    self.add_console_line(format!(">>> Hash error: {}", e));
                }
            }
        }

        // Check if source thread is done
        let source_done = self
            .hash_thread_source
            .as_ref()
            .map(|h| h.is_finished())
            .unwrap_or(true);

        if source_done && !self.source_hashes.is_empty() {
            // Collect summary lines first to avoid borrow conflict
            let summary_lines: Vec<String> = self.source_hashes.iter()
                .map(|fh| format!("  {} | SHA-256: {} | {} bytes", fh.relative_path, fh.hash, fh.size))
                .collect();
            let count = self.source_hashes.len();

            self.add_console_line(String::new());
            self.add_console_line(">>> Source File Hash Summary:".to_string());
            for line in summary_lines {
                self.add_console_line(line);
            }
            self.log(&format!("Source file hashing complete: {} files hashed", count));

            self.hash_thread_source = None;
            self.hash_progress_rx = None;
            
            // Save log to history after hashing is complete
            self.save_log_to_history();
            
            self.state = AppState::Idle;
        }
    }

    fn stop_operation(&mut self) {
        if let Some(mut child) = self.robocopy_child.take() {
            let _ = child.kill();
            self.log("Operation cancelled by user");
            self.add_console_line(">>> Operation cancelled by user".to_string());
        }
        self.state = AppState::Idle;
        self.console_rx = None;
        self.output_thread = None;
    }

    fn export_log(&self) {
        // Build default filename: graft_YYYY-MM-DD[_TICKET].txt
        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let default_filename = if !self.aft_ticket_number.is_empty() {
            let sanitized_ticket = self.aft_ticket_number.trim().replace(' ', "_");
            format!("graft_{}_{}.txt", date_str, sanitized_ticket)
        } else {
            format!("graft_{}.txt", date_str)
        };

        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Text files", &["txt"])
            .add_filter("Log files", &["log"])
            .set_file_name(&default_filename)
            .save_file()
        {
            let content = self.build_log_content();
            
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("Failed to save log: {}", e);
            }
        }
    }

    /// Build the log content from current transfer data
    fn build_log_content(&self) -> String {
        let mut content = String::new();
        let stats = &self.transfer_stats;

        // === Transfer Summary (top of log) ===
        content.push_str("=== TRANSFER SUMMARY ===\n");
        content.push_str(&format!("Date: {}\n", Local::now().format("%Y-%m-%d %H:%M:%S")));
        
        // Add username
        if let Some(username) = HistoryEntry::get_username() {
            content.push_str(&format!("Username: {}\n", username));
        }
        
        if !self.aft_ticket_number.is_empty() {
            content.push_str(&format!("AFT Ticket Number: {}\n", self.aft_ticket_number));
        }
        content.push_str(&format!("Source: {}\n", self.source_path));
        content.push_str(&format!("Destination: {}\n", self.destination_path));
        content.push_str(&format!("Command: {}\n", self.options.build_command_string(&self.source_path, &self.destination_path)));
        content.push_str(&format!("Robocopy Exit Code: {}\n", stats.robocopy_exit_code));
        content.push('\n');

        // === Transfer Statistics ===
        content.push_str("=== TRANSFER STATISTICS ===\n");
        content.push_str(&format!("Total Files:      {}\n", stats.files_total));
        content.push_str(&format!("Files Copied:     {}\n", stats.files_copied));
        content.push_str(&format!("Files Skipped:    {}\n", stats.files_skipped));
        content.push_str(&format!("Files Mismatched: {}\n", stats.files_mismatch));
        content.push_str(&format!("Files FAILED:     {}\n", stats.files_failed));
        content.push_str(&format!("Files Extras:     {}\n", stats.files_extras));
        content.push('\n');
        content.push_str(&format!("Total Dirs:       {}\n", stats.dirs_total));
        content.push_str(&format!("Dirs Copied:      {}\n", stats.dirs_copied));
        content.push_str(&format!("Dirs Skipped:     {}\n", stats.dirs_skipped));
        content.push_str(&format!("Dirs FAILED:      {}\n", stats.dirs_failed));
        content.push_str(&format!("Dirs Extras:      {}\n", stats.dirs_extras));
        content.push('\n');
        if !stats.bytes_total.is_empty() {
            content.push_str(&format!("Total Bytes:      {}\n", stats.bytes_total));
            content.push_str(&format!("Bytes Copied:     {}\n", stats.bytes_copied));
            if !stats.bytes_failed.is_empty() {
                content.push_str(&format!("Bytes FAILED:     {}\n", stats.bytes_failed));
            }
        }
        if !stats.speed.is_empty() {
            content.push_str(&format!("Transfer Speed:   {}\n", stats.speed));
        }
        content.push('\n');

        // Source file hash summary
        if !self.source_hashes.is_empty() {
            content.push_str("=== SOURCE FILE HASHES ===\n");
            content.push_str(&format_hash_results(&self.source_hashes));
            content.push('\n');
        }

        content.push_str("=== DETAILED LOG ===\n");
        content.push_str(&self.log_entries.join("\n"));
        content.push('\n');
        
        content
    }

    /// Save the current log to the history entry and automatically to disk
    fn save_log_to_history(&mut self) {
        if let Some(entry_id) = self.current_entry_id {
            let log_content = self.build_log_content();
            
            // Update history entry with log content
            let ticket = (!self.aft_ticket_number.is_empty()).then(|| self.aft_ticket_number.clone());
            self.history.set_log_content(entry_id, log_content, ticket);
            
            // Get the entry and save log to disk
            if let Some(entry) = self.history.get_entry_mut(entry_id) {
                match entry.save_log_to_disk() {
                    Ok(path) => {
                        self.log(&format!("Log saved to: {}", path.display()));
                        self.add_console_line(format!(">>> Log saved to: {}", path.display()));
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to auto-save log: {}", e);
                        self.log(&error_msg);
                        self.add_console_line(format!(">>> {}", error_msg));
                        eprintln!("{}", error_msg);
                    }
                }
            }
            
            // Save updated history
            let _ = self.history.save();
        }
    }
}

impl eframe::App for GraftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check async operations
        self.check_robocopy_done();
        self.check_hashing_done();

        // Request repaint if running
        if self.state != AppState::Idle {
            ctx.request_repaint();
        }

        self.render_destructive_warning(ctx);
        self.render_about_dialog(ctx);

        // Menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Export Log...").clicked() {
                        self.export_log();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });
            });
        });

        // Top panel with paths
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("GRAFT - Graphical Robocopy Assured File Transfer Tool");
            ui.add_space(8.0);

            // Source path row
            ui.horizontal(|ui| {
                ui.label("Source:        ");
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.source_path = path.to_string_lossy().to_string();
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.source_path)
                        .desired_width(ui.available_width())
                        .hint_text("Select source folder or enter path..."),
                );
            });

            ui.add_space(4.0);

            // Destination path row
            ui.horizontal(|ui| {
                ui.label("Destination:");
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.destination_path = path.to_string_lossy().to_string();
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.destination_path)
                        .desired_width(ui.available_width())
                        .hint_text("Select destination folder or enter path..."),
                );
            });

            ui.add_space(4.0);

            // AFT Ticket Number row
            ui.horizontal(|ui| {
                ui.label("AFT Ticket:    ");
                ui.add(
                    egui::TextEdit::singleline(&mut self.aft_ticket_number)
                        .desired_width(120.0)
                        .hint_text("e.g. TT1234"),
                );
            });

            ui.add_space(8.0);

            // Command preview
            ui.horizontal(|ui| {
                ui.label("Command:");
                let cmd = self.options.build_command_string(&self.source_path, &self.destination_path);
                ui.add(
                    egui::TextEdit::singleline(&mut cmd.clone())
                        .desired_width(ui.available_width() - 10.0)
                        .interactive(false),
                );
            });

            ui.add_space(8.0);

            // Action buttons
            ui.horizontal(|ui| {
                let can_run = self.state == AppState::Idle
                    && !self.source_path.is_empty()
                    && !self.destination_path.is_empty();

                if self.state == AppState::Idle {
                    if ui
                        .add_enabled(can_run, egui::Button::new("▶ Run Robocopy"))
                        .clicked()
                    {
                        self.request_start_robocopy();
                    }
                } else {
                    if ui.button("⏹ Stop").clicked() {
                        self.stop_operation();
                    }
                }

                ui.separator();

                ui.checkbox(&mut self.enable_hashing, "Include Source File Hash (SHA-256)");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match self.state {
                        AppState::Idle => {
                            ui.label("Ready");
                        }
                        AppState::Running => {
                            ui.spinner();
                            ui.label("Running robocopy...");
                        }
                        AppState::Hashing => {
                            ui.spinner();
                            ui.label(&self.hash_progress_text);
                        }
                    }
                });
            });

            ui.add_space(4.0);
        });

        // Bottom panel with log
        egui::TopBottomPanel::bottom("log_panel")
            .resizable(true)
            .min_height(100.0)
            .default_height(150.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Log");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Export Log").clicked() {
                            self.export_log();
                        }
                        ui.add_space(4.0);
                        if ui.button("Clear").clicked() {
                            self.log_entries.clear();
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for entry in &self.log_entries {
                            ui.label(entry);
                        }
                    });
            });

        // Right panel with console
        egui::SidePanel::right("console_panel")
            .resizable(true)
            .min_width(300.0)
            .default_width(700.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Console Output");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Clear").clicked() {
                            self.clear_console();
                        }
                    });
                });
                ui.separator();

                let mut scroll_area = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true);

                if self.console_scroll_to_bottom {
                    scroll_area = scroll_area.scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                    );
                    self.console_scroll_to_bottom = false;
                }

                scroll_area.show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for line in &self.console_output {
                        ui.label(line);
                    }
                });
            });

        // Central panel with options/history tabs
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, MainTab::Options, "⚙ Options");
                ui.selectable_value(&mut self.current_tab, MainTab::History, "📜 History");
            });
            ui.separator();

            match self.current_tab {
                MainTab::Options => self.render_options_tab(ui),
                MainTab::History => self.render_history_tab(ui),
            }
        });
    }
}

impl GraftApp {
    fn render_options_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Preset selection
            ui.heading("Presets");
            ui.add_space(4.0);

            let presets = [
                PresetGroup::LargeFilesWan,
                PresetGroup::MirrorWithMetadata,
                PresetGroup::CopyAllPreserve,
                PresetGroup::IncrementalBackup,
                PresetGroup::QuickCopy,
                PresetGroup::None,
            ];

            let current_preset = self.options.current_preset.clone();
            egui::ComboBox::from_label("Select Preset")
                .selected_text(current_preset.name())
                .show_ui(ui, |ui| {
                    for preset in &presets {
                        if ui
                            .selectable_label(
                                self.options.current_preset == *preset,
                                preset.name(),
                            )
                            .clicked()
                        {
                            self.options.apply_preset(preset.clone());
                        }
                    }
                });

            ui.label(
                egui::RichText::new(current_preset.description())
                    .small()
                    .color(egui::Color32::GRAY),
            );

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            let destructive = self.destructive_option_labels();
            if !destructive.is_empty() {
                ui.label(
                    egui::RichText::new("WARNING: Destructive options enabled")
                        .color(egui::Color32::from_rgb(255, 120, 80))
                        .strong(),
                );
                for label in destructive {
                    ui.label(
                        egui::RichText::new(format!("- {}", label))
                            .color(egui::Color32::from_rgb(255, 120, 80)),
                    );
                }
                ui.add_space(8.0);
            }

            // Options in collapsible sections
            ui.columns(2, |columns| {
                // Left column
                columns[0].collapsing("📁 Copy Options", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_subdirs);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_subdirs_empty);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_levels);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_restartable);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_backup);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_unbuffered);
                });

                columns[0].add_space(8.0);

                columns[0].collapsing("📋 File Selection", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_all);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_flags);
                    Self::render_option_checkbox_static(ui, &mut self.options.dir_copy_flags);
                    Self::render_option_checkbox_static(ui, &mut self.options.sec_copy);
                    Self::render_option_checkbox_static(ui, &mut self.options.copy_timestamps);
                    Self::render_option_checkbox_static(ui, &mut self.options.purge);
                    Self::render_option_checkbox_static(ui, &mut self.options.mirror);
                    Self::render_option_checkbox_static(ui, &mut self.options.move_files);
                    Self::render_option_checkbox_static(ui, &mut self.options.move_files_dirs);
                });

                columns[0].add_space(8.0);

                columns[0].collapsing("🏷 Attributes", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.attr_add);
                    Self::render_option_checkbox_static(ui, &mut self.options.attr_remove);
                    Self::render_option_checkbox_static(ui, &mut self.options.create_tree);
                });

                // Right column
                columns[1].collapsing("🔄 Retry Options", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.retry_count);
                    Self::render_option_checkbox_static(ui, &mut self.options.retry_wait);
                });

                columns[1].add_space(8.0);

                columns[1].collapsing("📝 Logging Options", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.log_verbose);
                    Self::render_option_checkbox_static(ui, &mut self.options.log_timestamps);
                    Self::render_option_checkbox_static(ui, &mut self.options.log_full_path);
                    Self::render_option_checkbox_static(ui, &mut self.options.log_bytes);
                    Self::render_option_checkbox_static(ui, &mut self.options.no_progress);
                    Self::render_option_checkbox_static(ui, &mut self.options.log_eta);
                });

                columns[1].add_space(8.0);

                columns[1].collapsing("🔍 File Filters", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.exclude_changed);
                    Self::render_option_checkbox_static(ui, &mut self.options.exclude_newer);
                    Self::render_option_checkbox_static(ui, &mut self.options.exclude_older);
                    Self::render_option_checkbox_static(ui, &mut self.options.exclude_extra);
                    Self::render_option_checkbox_static(ui, &mut self.options.exclude_lonely);
                    Self::render_option_checkbox_static(ui, &mut self.options.include_same);
                    Self::render_option_checkbox_static(ui, &mut self.options.include_modified);
                });

                columns[1].add_space(8.0);

                columns[1].collapsing("⚡ Performance", |ui| {
                    Self::render_option_checkbox_static(ui, &mut self.options.multi_thread);
                    Self::render_option_checkbox_static(ui, &mut self.options.inter_packet_gap);
                });
            });
            
            // Check if options have been manually changed
            if self.options.current_preset != PresetGroup::None && !self.options.matches_current_preset() {
                self.options.current_preset = PresetGroup::None;
            }
        });
    }

    fn render_option_checkbox_static(ui: &mut egui::Ui, opt: &mut RobocopyOption) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut opt.enabled, &opt.name);
            if opt.has_value && opt.enabled {
                ui.add(egui::TextEdit::singleline(&mut opt.value).desired_width(60.0));
            }
        });
        ui.label(
            egui::RichText::new(&opt.description)
                .small()
                .color(egui::Color32::GRAY),
        );
        ui.add_space(4.0);
    }

    fn render_history_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Command History");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear Unsaved").clicked() {
                    self.history.entries.retain(|e| e.saved);
                    let _ = self.history.save();
                }
            });
        });

        ui.add_space(8.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Saved commands
            ui.collapsing("⭐ Saved Commands", |ui| {
                let saved: Vec<_> = self.history.saved_entries().into_iter().cloned().collect();
                if saved.is_empty() {
                    ui.label("No saved commands");
                } else {
                    for entry in saved {
                        self.render_history_entry(ui, &entry);
                    }
                }
            });

            ui.add_space(8.0);

            // Recent commands
            ui.collapsing("🕒 Recent Commands", |ui| {
                let recent: Vec<_> = self.history.recent_entries().into_iter().cloned().collect();
                if recent.is_empty() {
                    ui.label("No recent commands");
                } else {
                    for entry in recent {
                        self.render_history_entry(ui, &entry);
                    }
                }
            });
        });
    }

    fn render_history_entry(&mut self, ui: &mut egui::Ui, entry: &HistoryEntry) {
        let id = entry.id;
        let is_renaming = self.selected_history_id == Some(id);
        
        egui::Frame::default()
            .inner_margin(8.0)
            .outer_margin(egui::Margin::symmetric(0, 4))
            .fill(ui.style().visuals.extreme_bg_color)
            .corner_radius(4)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Star/save button
                    let star = if entry.saved { "⭐" } else { "☆" };
                    if ui.button(star).clicked() {
                        self.history.toggle_save(id);
                        let _ = self.history.save();
                    }

                    // Entry info
                    ui.vertical(|ui| {
                        if is_renaming {
                            // Show rename text field
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.rename_buffer)
                                    .desired_width(200.0)
                                    .hint_text("Enter name...")
                            );
                            
                            // Auto-focus the text field
                            if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                // Save the new name
                                self.history.set_name(id, self.rename_buffer.clone());
                                let _ = self.history.save();
                                self.selected_history_id = None;
                                self.rename_buffer.clear();
                            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                // Cancel renaming
                                self.selected_history_id = None;
                                self.rename_buffer.clear();
                            }
                        } else {
                            // Show entry name (clickable to rename)
                            let name_response = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(entry.display_name()).strong()
                                ).sense(egui::Sense::click())
                            );
                            if name_response.double_clicked() {
                                self.selected_history_id = Some(id);
                                self.rename_buffer = entry.name.clone().unwrap_or_default();
                            }
                            if name_response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                            }
                        }
                        
                        ui.label(
                            egui::RichText::new(&entry.command)
                                .small()
                                .color(egui::Color32::GRAY),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Delete button
                        if ui.button("🗑").clicked() {
                            self.history.delete_entry(id);
                            let _ = self.history.save();
                        }
                        
                        // Export Log button (only if log content exists)
                        if entry.log_content.is_some() {
                            if ui.button("📄").on_hover_text("Export Log").clicked() {
                                self.export_history_log(entry);
                            }
                        }
                        
                        // Rename button
                        if ui.button("✏").on_hover_text("Rename").clicked() {
                            self.selected_history_id = Some(id);
                            self.rename_buffer = entry.name.clone().unwrap_or_default();
                        }

                        // Load button
                        if ui.button("Load").clicked() {
                            self.source_path = entry.source.clone();
                            self.destination_path = entry.destination.clone();
                            self.options = entry.options.clone();
                        }

                        // Run button
                        if ui
                            .add_enabled(self.state == AppState::Idle, egui::Button::new("▶ Run"))
                            .clicked()
                        {
                            self.source_path = entry.source.clone();
                            self.destination_path = entry.destination.clone();
                            self.options = entry.options.clone();
                            self.request_start_robocopy();
                        }
                    });
                });
            });
    }

    /// Export a log from a history entry
    fn export_history_log(&self, entry: &HistoryEntry) {
        if let Some(ref log_content) = entry.log_content {
            let default_filename = entry.generate_log_filename();
            
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Text files", &["txt"])
                .add_filter("Log files", &["log"])
                .set_file_name(&default_filename)
                .save_file()
            {
                if let Err(e) = std::fs::write(&path, log_content) {
                    eprintln!("Failed to export log: {}", e);
                }
            }
        }
    }

    fn render_destructive_warning(&mut self, ctx: &egui::Context) {
        if !self.show_destructive_warning {
            return;
        }

        egui::Window::new("Destructive Options Enabled")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(&self.destructive_warning_text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.show_destructive_warning = false;
                    }
                    if ui.button("Run anyway").clicked() {
                        self.show_destructive_warning = false;
                        if self.state == AppState::Idle {
                            self.start_robocopy();
                        }
                    }
                });
            });
    }

    fn render_about_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }

        egui::Window::new("About GRAFT")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("GRAFT");
                    ui.label("Graphical Robocopy Assured File Transfer Tool");
                    ui.add_space(4.0);
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(12.0);
                    if ui.button("OK").clicked() {
                        self.show_about = false;
                    }
                    ui.add_space(4.0);
                });
            });
    }
}
