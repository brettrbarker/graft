//! Main application GUI module

use crate::hasher::{
    collect_files, format_hash_results, hash_directory, hash_file, compare_hashes,
    FileHash, HashProgress,
};
use crate::history::{CommandHistory, HistoryEntry};
use crate::robocopy::{PresetGroup, RobocopyOption, RobocopyOptions};
use chrono::Local;
use eframe::egui;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

#[derive(PartialEq, Clone, Copy)]
enum SourceSelectionMode {
    Folder,
    File,
}

/// Console output line type for syntax highlighting
#[derive(Clone, Debug)]
enum ConsoleLineType {
    Normal,
    Command,      // Command being executed
    Success,      // Successful operations
    Warning,      // Warnings
    Error,        // Errors
    Summary,      // Summary statistics
}

/// Console output line with metadata
#[derive(Clone, Debug)]
struct ConsoleOutputLine {
    text: String,
    line_type: ConsoleLineType,
}

impl ConsoleOutputLine {
    fn new(text: String, line_type: ConsoleLineType) -> Self {
        Self { text, line_type }
    }
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
    source_mode: SourceSelectionMode,

    // Robocopy options
    options: RobocopyOptions,

    // History
    history: CommandHistory,
    selected_history_id: Option<u64>,
    rename_buffer: String,
    current_entry_id: Option<u64>,

    // Console output
    console_output: Vec<ConsoleOutputLine>,
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
    enable_destination_hashing: bool,
    show_destination_hash_warning: bool,
    source_hashes: Vec<FileHash>,
    destination_hashes: Vec<FileHash>,
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
    hash_thread_destination: Option<JoinHandle<Result<Vec<FileHash>, String>>>,

    // Cancel flag for operations
    cancel_requested: Arc<AtomicBool>,
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

        let source_mode = if !source_path.is_empty() && Path::new(&source_path).is_file() {
            SourceSelectionMode::File
        } else {
            SourceSelectionMode::Folder
        };
        
        Self {
            source_path,
            destination_path,
            source_mode,
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
            enable_destination_hashing: false,
            show_destination_hash_warning: false,
            source_hashes: Vec::new(),
            destination_hashes: Vec::new(),
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
            hash_thread_destination: None,
            cancel_requested: Arc::new(AtomicBool::new(false)),
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
        let line_type = Self::detect_line_type(&line);
        self.console_output.push(ConsoleOutputLine::new(line, line_type));
        self.console_scroll_to_bottom = true;
        // Keep console buffer reasonable
        if self.console_output.len() > 10000 {
            self.console_output.drain(0..5000);
        }
    }

    /// Detect the line type based on content
    fn detect_line_type(line: &str) -> ConsoleLineType {
        let trimmed = line.trim();
        
        // Command lines start with ">>>"
        if trimmed.starts_with(">>>") {
            return ConsoleLineType::Command;
        }
        
        // Errors
        if trimmed.contains("[ERROR]") || trimmed.contains("Error:") || trimmed.starts_with("❌") {
            return ConsoleLineType::Error;
        }
        
        // Warnings
        if trimmed.contains("WARNING") || trimmed.starts_with("⚠") {
            return ConsoleLineType::Warning;
        }
        
        // Success indicators
        if trimmed.contains("✓") || trimmed.contains("Success") || trimmed.contains("completed successfully") {
            return ConsoleLineType::Success;
        }
        
        // Summary lines (robocopy statistics)
        if trimmed.starts_with("Dirs :") || trimmed.starts_with("Files :") || 
           trimmed.starts_with("Bytes :") || trimmed.starts_with("Times :") || 
           trimmed.starts_with("Speed :") || trimmed.contains("Total") && trimmed.contains("Copied") {
            return ConsoleLineType::Summary;
        }
        
        ConsoleLineType::Normal
    }

    fn clear_console(&mut self) {
        self.console_output.clear();
    }

    fn cancel_operation(&mut self) {
        self.cancel_requested.store(true, Ordering::Relaxed);
        self.log("Cancel requested...");
        self.add_console_line("⚠ Operation cancelled by user".to_string());
        
        // Kill robocopy process if running
        if let Some(mut child) = self.robocopy_child.take() {
            let _ = child.kill();
            self.log("Terminated robocopy process");
        }
        
        // Note: Hash threads will check cancel_requested and exit gracefully
        self.state = AppState::Idle;
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

    fn resolved_source_and_filter(&self) -> Result<(String, Option<String>), String> {
        match self.source_mode {
            SourceSelectionMode::Folder => Ok((self.source_path.clone(), None)),
            SourceSelectionMode::File => {
                let source = Path::new(&self.source_path);
                let parent = source
                    .parent()
                    .ok_or_else(|| "Source file must have a parent directory".to_string())?;
                let file_name = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "Source file name is invalid".to_string())?;
                Ok((
                    parent.to_string_lossy().to_string(),
                    Some(file_name.to_string()),
                ))
            }
        }
    }

    fn build_current_command_string(&self) -> String {
        match self.resolved_source_and_filter() {
            Ok((source, file_filter)) => self
                .options
                .build_command_string_with_filter(&source, &self.destination_path, file_filter.as_deref()),
            Err(_) => self
                .options
                .build_command_string(&self.source_path, &self.destination_path),
        }
    }

    fn build_current_args(&self) -> Result<Vec<String>, String> {
        let (source, file_filter) = self.resolved_source_and_filter()?;
        Ok(match file_filter {
            Some(filter) => self
                .options
                .build_args_with_filter(&source, &self.destination_path, Some(&filter)),
            None => self.options.build_args(&source, &self.destination_path),
        })
    }

    fn source_file_name(&self) -> Option<String> {
        if self.source_mode != SourceSelectionMode::File {
            return None;
        }

        Path::new(&self.source_path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
    }

    fn destination_file_target_path(&self) -> Option<PathBuf> {
        let file_name = self.source_file_name()?;
        Some(PathBuf::from(&self.destination_path).join(file_name))
    }

    fn update_source_mode_from_path(&mut self) {
        let path = Path::new(&self.source_path);
        if path.is_file() {
            self.source_mode = SourceSelectionMode::File;
        } else if path.is_dir() {
            self.source_mode = SourceSelectionMode::Folder;
        }
    }

    fn file_mode_incompatible_options(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.source_mode != SourceSelectionMode::File {
            return labels;
        }

        if self.options.copy_subdirs.enabled {
            labels.push("Copy Subdirectories (/S)");
        }
        if self.options.copy_subdirs_empty.enabled {
            labels.push("Copy Empty Subdirs (/E)");
        }
        if self.options.copy_levels.enabled {
            labels.push("Copy Levels (/LEV)");
        }
        if self.options.mirror.enabled {
            labels.push("Mirror Mode (/MIR)");
        }
        if self.options.purge.enabled {
            labels.push("Purge Destination (/PURGE)");
        }

        labels
    }

    fn disable_file_mode_incompatible_options(&mut self) -> Vec<&'static str> {
        let incompatible = self.file_mode_incompatible_options();
        if incompatible.is_empty() {
            return incompatible;
        }

        self.options.copy_subdirs.enabled = false;
        self.options.copy_subdirs_empty.enabled = false;
        self.options.copy_levels.enabled = false;
        self.options.mirror.enabled = false;
        self.options.purge.enabled = false;

        incompatible
    }

    fn spawn_single_file_hashing(
        file_path: PathBuf,
        display_name: String,
        progress_tx: mpsc::Sender<HashProgress>,
        cancel_flag: Arc<AtomicBool>,
    ) -> JoinHandle<Result<Vec<FileHash>, String>> {
        thread::spawn(move || {
            if cancel_flag.load(Ordering::Relaxed) {
                let msg = "Cancelled by user".to_string();
                let _ = progress_tx.send(HashProgress::Error(msg.clone()));
                return Err(msg);
            }

            if !file_path.exists() {
                let msg = format!("File does not exist: {}", file_path.display());
                let _ = progress_tx.send(HashProgress::Error(msg.clone()));
                return Err(msg);
            }

            if !file_path.is_file() {
                let msg = format!("Path is not a file: {}", file_path.display());
                let _ = progress_tx.send(HashProgress::Error(msg.clone()));
                return Err(msg);
            }

            let _ = progress_tx.send(HashProgress::Starting(1));
            let _ = progress_tx.send(HashProgress::FileStarted(display_name.clone()));

            let hash = hash_file(&file_path)?;
            let size = std::fs::metadata(&file_path)
                .map(|m| m.len())
                .unwrap_or(0);

            let file_hash = FileHash {
                path: file_path,
                relative_path: display_name,
                hash,
                size,
            };

            let _ = progress_tx.send(HashProgress::FileComplete(file_hash.clone()));
            let _ = progress_tx.send(HashProgress::Complete(vec![file_hash.clone()]));
            Ok(vec![file_hash])
        })
    }

    fn request_start_robocopy(&mut self) {
        // Validate paths first
        if let Err(error) = self.validate_paths() {
            self.log(&format!("Validation Error: {}", error));
            self.add_console_line(format!("❌ Error: {}", error));
            return;
        }

        let disabled_options = self.disable_file_mode_incompatible_options();
        if !disabled_options.is_empty() {
            self.log("File source mode: disabled incompatible options");
            self.add_console_line("⚠ File source mode: ignoring incompatible options: ".to_string());
            for label in disabled_options {
                self.add_console_line(format!("⚠ {}", label));
            }
        }

        let option_errors = self.options.validate_enabled_options();
        if !option_errors.is_empty() {
            self.log("Validation Error: invalid custom option values");
            self.add_console_line("❌ Error: Invalid custom option values:".to_string());
            for error in option_errors {
                self.log(&format!("Validation Error: {}", error));
                self.add_console_line(format!("❌ {}", error));
            }
            return;
        }

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

    /// Validate source and destination paths
    fn validate_paths(&self) -> Result<(), String> {
        if self.source_path.is_empty() {
            return Err("Source path cannot be empty".to_string());
        }

        if self.destination_path.is_empty() {
            return Err("Destination path cannot be empty".to_string());
        }

        // Normalize paths for comparison
        let source = Path::new(&self.source_path);
        let dest = Path::new(&self.destination_path);

        // Check if source exists (unless it's a dry run)
        if !self.options.dry_run.enabled && !source.exists() {
            return Err(format!("Source path does not exist: {}", self.source_path));
        }

        // Source can be a folder or a single file, depending on the selected mode.
        if source.exists() {
            match self.source_mode {
                SourceSelectionMode::Folder if !source.is_dir() => {
                    return Err("Source must be a folder when Source Type is set to Folder".to_string());
                }
                SourceSelectionMode::File if !source.is_file() => {
                    return Err("Source must be a file when Source Type is set to File".to_string());
                }
                _ => {}
            }
        }

        // Destination must always be a directory if it already exists.
        if dest.exists() && !dest.is_dir() {
            return Err("Destination must be a folder".to_string());
        }

        // Check if paths are the same
        if let (Ok(src_canon), Ok(dst_canon)) = (source.canonicalize(), dest.canonicalize()) {
            if src_canon == dst_canon {
                return Err("Source and destination cannot be the same path".to_string());
            }
        } else if self.source_path == self.destination_path {
            // Fallback comparison if canonicalize fails
            return Err("Source and destination cannot be the same path".to_string());
        }

        // Directory nesting checks only apply when source is a directory.
        if self.source_mode == SourceSelectionMode::Folder {
            if dest.starts_with(source) {
                return Err("Destination cannot be inside the source directory".to_string());
            }

            if source.starts_with(dest) && (self.options.mirror.enabled || self.options.purge.enabled) {
                return Err("Source cannot be inside destination when using Mirror or Purge options".to_string());
            }
        }

        // Check path length (Windows MAX_PATH is 260, but we'll be conservative)
        if self.source_path.len() > 250 || self.destination_path.len() > 250 {
            return Err("Path length exceeds safe limits (max 250 characters recommended)".to_string());
        }

        // Check for invalid characters in paths (Windows specific)
        let invalid_chars = ['<', '>', '"', '|', '?', '*'];
        for ch in invalid_chars {
            if self.source_path.contains(ch) || self.destination_path.contains(ch) {
                return Err(format!("Path contains invalid character: '{}'", ch));
            }
        }

        Ok(())
    }

    fn start_robocopy(&mut self) {
        // Validation is done in request_start_robocopy, so we can proceed
        // Reset cancel flag for new operation
        self.cancel_requested.store(false, Ordering::Relaxed);
        
        let command = self.build_current_command_string();
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
        let args = match self.build_current_args() {
            Ok(args) => args,
            Err(e) => {
                self.log(&format!("Failed to build robocopy arguments: {}", e));
                self.add_console_line(format!("Error: Failed to build robocopy arguments: {}", e));
                return;
            }
        };

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
            let trimmed = line.text.trim();
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

                // Start hashing if any hash option is enabled and robocopy succeeded (exit code < 8)
                let last_exit_code = self.transfer_stats.robocopy_exit_code;
                let hash_requested = self.enable_hashing || self.enable_destination_hashing;

                if hash_requested && last_exit_code < 8 {
                    self.start_hashing();
                } else if hash_requested && last_exit_code >= 8 {
                    self.log("Skipping hash operations due to robocopy errors");
                    self.add_console_line(">>> Skipping hash operations due to robocopy errors".to_string());
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
        self.source_hashes.clear();
        self.destination_hashes.clear();
        self.hash_progress_text = "Initializing...".to_string();
        self.hash_files_processed = 0;
        self.hash_files_total = 0;
        self.hash_thread_source = None;
        self.hash_thread_destination = None;

        let (tx, rx) = mpsc::channel::<HashProgress>();
        self.hash_progress_rx = Some(rx);

        let cancel_flag = Arc::clone(&self.cancel_requested);

        if self.enable_hashing {
            self.log("Starting source file hashing...");
            self.add_console_line(String::new());
            self.add_console_line(">>> Starting source file hashing...".to_string());

            if self.source_mode == SourceSelectionMode::File {
                let source_path = PathBuf::from(&self.source_path);
                let display_name = self
                    .source_file_name()
                    .unwrap_or_else(|| source_path.to_string_lossy().to_string());
                self.hash_thread_source = Some(Self::spawn_single_file_hashing(
                    source_path,
                    display_name,
                    tx,
                    cancel_flag,
                ));
            } else {
                let source_path = PathBuf::from(&self.source_path);
                self.hash_thread_source = Some(hash_directory(&source_path, tx, cancel_flag));
            }
        } else if self.enable_destination_hashing {
            self.log("Starting destination file hashing...");
            self.add_console_line(String::new());
            self.add_console_line(">>> Starting destination file hashing...".to_string());

            if self.source_mode == SourceSelectionMode::File {
                if let Some(destination_path) = self.destination_file_target_path() {
                    let display_name = self
                        .source_file_name()
                        .unwrap_or_else(|| destination_path.to_string_lossy().to_string());
                    self.hash_thread_destination = Some(Self::spawn_single_file_hashing(
                        destination_path,
                        display_name,
                        tx,
                        cancel_flag,
                    ));
                } else {
                    self.log("Destination hash setup failed: source file name is invalid");
                    self.add_console_line(">>> Destination hash setup failed: source file name is invalid".to_string());
                    self.state = AppState::Idle;
                    return;
                }
            } else {
                let destination_path = PathBuf::from(&self.destination_path);
                self.hash_thread_destination = Some(hash_directory(&destination_path, tx, cancel_flag));
            }
        }

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
                    // Determine if hashing source or destination
                    if self.hash_thread_destination.is_none() {
                        self.hash_progress_text = format!("Hashing {} source files...", self.hash_files_total);
                    } else {
                        self.hash_progress_text = format!("Hashing {} destination files...", self.hash_files_total);
                    }
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
                    if self.hash_thread_destination.is_none() {
                        // Source hashing complete
                        self.source_hashes = hashes;
                        self.add_console_line(format!(
                            ">>> Source hashing complete: {} files",
                            self.source_hashes.len()
                        ));
                    } else {
                        // Destination hashing complete
                        self.destination_hashes = hashes;
                        self.add_console_line(format!(
                            ">>> Destination hashing complete: {} files",
                            self.destination_hashes.len()
                        ));
                    }
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

        if source_done && self.hash_thread_source.is_some() {
            if let Some(handle) = self.hash_thread_source.take() {
                match handle.join() {
                    Ok(Ok(hashes)) => {
                        if self.source_hashes.is_empty() {
                            self.source_hashes = hashes;
                        }
                    }
                    Ok(Err(e)) => {
                        self.log(&format!("Source hashing failed: {}", e));
                        self.add_console_line(format!(">>> Source hashing failed: {}", e));
                    }
                    Err(_) => {
                        self.log("Source hashing thread panicked");
                        self.add_console_line(">>> Source hashing thread panicked".to_string());
                    }
                }
            }

            if self.source_hashes.is_empty() {
                self.add_console_line(">>> Source hashing complete: 0 files".to_string());
                self.log("Source file hashing complete: 0 files hashed");
            } else {
                let count = self.source_hashes.len();
                let preview_count = count.min(25);
                let preview_lines: Vec<String> = self
                    .source_hashes
                    .iter()
                    .take(preview_count)
                    .map(|fh| {
                        format!(
                            "  {} | SHA-256: {} | {} bytes",
                            fh.relative_path,
                            fh.hash,
                            fh.size
                        )
                    })
                    .collect();

                self.add_console_line(String::new());
                self.add_console_line(">>> Source File Hash Summary:".to_string());
                self.add_console_line(format!("  Total hashed files: {}", count));
                self.add_console_line(format!("  Showing first {} files:", preview_count));
                for line in preview_lines {
                    self.add_console_line(line);
                }
                if count > preview_count {
                    self.add_console_line(format!(
                        "  ... {} additional files omitted from live console output",
                        count - preview_count
                    ));
                }
                self.log(&format!("Source file hashing complete: {} files hashed", count));
            }

            // If destination hashing is not enabled, source hashing is the terminal stage.
            if !self.enable_destination_hashing {
                self.hash_progress_rx = None;
                self.save_log_to_history();
                self.state = AppState::Idle;
                return;
            }

            // Start destination hashing if enabled
            if self.enable_destination_hashing {
                self.hash_progress_text = "Initializing destination hashing...".to_string();
                self.hash_files_processed = 0;
                self.hash_files_total = 0;
                self.destination_hashes.clear();
                
                self.add_console_line(String::new());
                self.add_console_line(">>> Starting destination file hashing...".to_string());
                self.log("Starting destination file hashing...");

                let (tx, rx) = mpsc::channel::<HashProgress>();
                self.hash_progress_rx = Some(rx);

                let cancel_flag = Arc::clone(&self.cancel_requested);
                if self.source_mode == SourceSelectionMode::File {
                    if let Some(dest_file_path) = self.destination_file_target_path() {
                        let display_name = self
                            .source_file_name()
                            .unwrap_or_else(|| dest_file_path.to_string_lossy().to_string());
                        self.hash_thread_destination = Some(Self::spawn_single_file_hashing(
                            dest_file_path,
                            display_name,
                            tx,
                            cancel_flag,
                        ));
                    } else {
                        self.log("Destination hash setup failed: source file name is invalid");
                        self.add_console_line(">>> Destination hash setup failed: source file name is invalid".to_string());
                        self.hash_progress_rx = None;
                        self.save_log_to_history();
                        self.state = AppState::Idle;
                        return;
                    }
                } else {
                    let dest_path = PathBuf::from(&self.destination_path);
                    self.hash_thread_destination = Some(hash_directory(&dest_path, tx, cancel_flag));
                }
                
                // Continue hashing state
                return;
            }
        }

        // Check if destination thread is done
        let dest_done = self
            .hash_thread_destination
            .as_ref()
            .map(|h| h.is_finished())
            .unwrap_or(true);

        if dest_done && self.hash_thread_destination.is_some() {
            if let Some(handle) = self.hash_thread_destination.take() {
                match handle.join() {
                    Ok(Ok(hashes)) => {
                        if self.destination_hashes.is_empty() {
                            self.destination_hashes = hashes;
                        }
                    }
                    Ok(Err(e)) => {
                        self.log(&format!("Destination hashing failed: {}", e));
                        self.add_console_line(format!(">>> Destination hashing failed: {}", e));
                    }
                    Err(_) => {
                        self.log("Destination hashing thread panicked");
                        self.add_console_line(">>> Destination hashing thread panicked".to_string());
                    }
                }
            }

            if self.destination_hashes.is_empty() {
                self.add_console_line(">>> Destination hashing complete: 0 files".to_string());
                self.log("Destination file hashing complete: 0 files hashed");
            } else {
                let count = self.destination_hashes.len();
                let preview_count = count.min(25);
                let preview_lines: Vec<String> = self
                    .destination_hashes
                    .iter()
                    .take(preview_count)
                    .map(|fh| {
                        format!(
                            "  {} | SHA-256: {} | {} bytes",
                            fh.relative_path,
                            fh.hash,
                            fh.size
                        )
                    })
                    .collect();

                self.add_console_line(String::new());
                self.add_console_line(">>> Destination File Hash Summary:".to_string());
                self.add_console_line(format!("  Total hashed files: {}", count));
                self.add_console_line(format!("  Showing first {} files:", preview_count));
                for line in preview_lines {
                    self.add_console_line(line);
                }
                if count > preview_count {
                    self.add_console_line(format!(
                        "  ... {} additional files omitted from live console output",
                        count - preview_count
                    ));
                }
                self.log(&format!("Destination file hashing complete: {} files hashed", count));
            }

            // Compare hashes if destination hashing was done
            if self.enable_destination_hashing && !self.source_hashes.is_empty() && !self.destination_hashes.is_empty() {
                self.add_console_line(String::new());
                self.add_console_line(">>> Comparing source and destination hashes...".to_string());
                let verification = compare_hashes(&self.source_hashes, &self.destination_hashes);
                
                self.add_console_line(String::new());
                self.add_console_line(">>> Hash Verification Report:".to_string());
                
                if verification.matched.is_empty() && verification.mismatched.is_empty() && 
                   verification.missing_in_dest.is_empty() && verification.extra_in_dest.is_empty() {
                    self.add_console_line("✓ All files matched perfectly!".to_string());
                    self.log("Hash verification: All files matched perfectly");
                } else {
                    self.add_console_line(format!("Summary: {}", verification.summary()));
                    
                    if !verification.matched.is_empty() {
                        self.add_console_line(format!("✓ Matched: {} files", verification.matched.len()));
                    }
                    if !verification.mismatched.is_empty() {
                        self.add_console_line(format!("✗ Mismatched: {} files", verification.mismatched.len()));
                        for (path, src_hash, dst_hash) in &verification.mismatched {
                            self.add_console_line(format!("  {} | Source: {} | Dest: {}",
                                path, &src_hash[..16], &dst_hash[..16]));
                        }
                    }
                    if !verification.missing_in_dest.is_empty() {
                        self.add_console_line(format!("⚠ Missing in destination: {} files", verification.missing_in_dest.len()));
                    }
                    if !verification.extra_in_dest.is_empty() {
                        self.add_console_line(format!("ℹ Extra in destination: {} files", verification.extra_in_dest.len()));
                    }
                    
                    self.log(&format!("Hash verification: {}", verification.summary()));
                }
            }

            self.hash_progress_rx = None;

            // Save log to history after hashing is complete
            self.save_log_to_history();

            self.state = AppState::Idle;
        }
    }

    fn export_log(&self) {
        let ticket = self.aft_ticket_number.trim();
        let ticket = if ticket.is_empty() { None } else { Some(ticket) };
        let default_filename = HistoryEntry::generate_log_filename_for_timestamp(Local::now(), ticket);

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
        content.push_str(&format!("Command: {}\n", self.build_current_command_string()));
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

        let file_status_entries = self.extract_file_status_entries();
        content.push_str("=== FILE LIST ===\n");
        if file_status_entries.is_empty() {
            content.push_str("No file-level status entries were captured for this run.\n");
            content.push('\n');
        } else {
            content.push_str("Status | File\n");
            content.push_str("-------|-----\n");
            for (status, file_path) in file_status_entries {
                content.push_str(&format!("{} | {}\n", status, file_path));
            }
            content.push('\n');
        }

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

    /// Extract copied or already-synced file entries from robocopy output.
    fn extract_file_status_entries(&self) -> Vec<(String, String)> {
        let status_markers = [
            ("NEW FILE", "Copied"),
            ("NEWER", "Copied"),
            ("OLDER", "Copied"),
            ("CHANGED", "Copied"),
            ("TWEAKED", "Copied"),
            ("SAME", "Already Synced"),
        ];

        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        for line in &self.console_output {
            let text = line.text.trim();
            if text.is_empty() || text.starts_with(">>>") {
                continue;
            }

            let upper = text.to_ascii_uppercase();

            for (marker, status) in status_markers {
                if let Some(idx) = upper.find(marker) {
                    let raw_path = text[idx + marker.len()..].trim();
                    let file_path = Self::normalize_robocopy_file_path(raw_path);
                    if file_path.is_empty() {
                        break;
                    }

                    let dedupe_key = format!("{}|{}", status, file_path.to_ascii_lowercase());
                    if seen.insert(dedupe_key) {
                        entries.push((status.to_string(), file_path));
                    }
                    break;
                }
            }
        }

        if entries.is_empty() {
            entries = self.build_fallback_synced_file_entries();
        }

        entries
    }

    /// Fallback when robocopy doesn't emit per-file SAME rows: if summary indicates
    /// all files were already in sync, enumerate source files as Already Synced.
    fn build_fallback_synced_file_entries(&self) -> Vec<(String, String)> {
        let stats = &self.transfer_stats;
        let fully_synced = stats.robocopy_exit_code == 0
            && stats.files_total > 0
            && stats.files_copied == 0
            && stats.files_mismatch == 0
            && stats.files_failed == 0;

        if !fully_synced || self.source_path.trim().is_empty() {
            return Vec::new();
        }

        if self.source_mode == SourceSelectionMode::File {
            if let Some(file_name) = self.source_file_name() {
                return vec![("Already Synced".to_string(), file_name)];
            }
            return Vec::new();
        }

        let source_root = PathBuf::from(&self.source_path);
        let files = match collect_files(&source_root) {
            Ok(files) => files,
            Err(_) => return Vec::new(),
        };

        let mut relative_paths: Vec<String> = files
            .into_iter()
            .filter_map(|path| {
                path.strip_prefix(&source_root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .collect();

        relative_paths.sort_by_key(|p| p.to_ascii_lowercase());

        relative_paths
            .into_iter()
            .map(|path| ("Already Synced".to_string(), path))
            .collect()
    }

    /// Normalize a file path column extracted from a robocopy output line.
    fn normalize_robocopy_file_path(raw: &str) -> String {
        let mut value = raw.trim();

        // Some robocopy rows include a leading size token before the path.
        loop {
            let mut parts = value.splitn(2, char::is_whitespace);
            let first = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("").trim_start();

            let looks_like_size = !first.is_empty()
                && first
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || ch == ',' || ch == '.');

            if looks_like_size && !rest.is_empty() {
                value = rest;
            } else {
                break;
            }
        }

        value.trim_matches('"').trim().to_string()
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
        self.render_destination_hash_warning(ctx);
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

            // Source path row with recent paths dropdown
            ui.horizontal(|ui| {
                ui.label("Source:        ");

                ui.label("Type:");
                ui.selectable_value(&mut self.source_mode, SourceSelectionMode::Folder, "Folder");
                ui.selectable_value(&mut self.source_mode, SourceSelectionMode::File, "File");

                if ui.button("Browse...").clicked() {
                    let selected = match self.source_mode {
                        SourceSelectionMode::Folder => rfd::FileDialog::new().pick_folder(),
                        SourceSelectionMode::File => rfd::FileDialog::new().pick_file(),
                    };

                    if let Some(path) = selected {
                        self.source_path = path.to_string_lossy().to_string();
                        self.update_source_mode_from_path();
                    }
                }
                
                // Recent paths dropdown
                let recent_sources = self.history.get_recent_source_paths().to_vec();
                if !recent_sources.is_empty() {
                    egui::ComboBox::from_id_salt("recent_sources")
                        .selected_text("Recent...")
                        .show_ui(ui, |ui| {
                            for recent_path in &recent_sources {
                                if ui.selectable_label(false, recent_path).clicked() {
                                    self.source_path = recent_path.clone();
                                    self.update_source_mode_from_path();
                                }
                            }
                        });
                }
                
                ui.add(
                    egui::TextEdit::singleline(&mut self.source_path)
                        .desired_width(ui.available_width())
                        .hint_text(match self.source_mode {
                            SourceSelectionMode::Folder => "Select source folder or enter path...",
                            SourceSelectionMode::File => "Select source file or enter path...",
                        }),
                );
            });

            ui.add_space(4.0);

            // Destination path row with recent paths dropdown
            ui.horizontal(|ui| {
                ui.label("Destination:");
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.destination_path = path.to_string_lossy().to_string();
                    }
                }
                
                // Recent paths dropdown
                let recent_dests = self.history.get_recent_dest_paths().to_vec();
                if !recent_dests.is_empty() {
                    egui::ComboBox::from_id_salt("recent_dests")
                        .selected_text("Recent...")
                        .show_ui(ui, |ui| {
                            for recent_path in &recent_dests {
                                if ui.selectable_label(false, recent_path).clicked() {
                                    self.destination_path = recent_path.clone();
                                }
                            }
                        });
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
                let cmd = self.build_current_command_string();
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
                    if ui.button("⏹ Cancel").clicked() {
                        self.cancel_operation();
                    }
                }

                ui.separator();

                ui.vertical(|ui| {
                    ui.checkbox(&mut self.enable_hashing, "Include Source File Hash (SHA-256)");

                    ui.horizontal(|ui| {
                        let checkbox_clicked = ui.checkbox(&mut self.enable_destination_hashing,
                            "Include Destination Hash Verification (SHA-256)").clicked();

                        if self.enable_destination_hashing && checkbox_clicked {
                            // Show warning when enabling
                            self.show_destination_hash_warning = true;
                        }

                        let info_clicked = ui
                            .small_button("ℹ")
                            .on_hover_text(
                                "Enables SHA-256 hashing of destination after transfer.\n\
                                If source hashing is also enabled, the app automatically compares source and destination hashes.\n\
                                This may take considerable time if destination is on a slow network connection."
                            )
                            .clicked();

                        if info_clicked {
                            self.show_destination_hash_warning = true;
                        }
                    });

                    if self.enable_hashing && self.enable_destination_hashing {
                        ui.label("Both hash options enabled: source and destination hashes will be compared automatically.");
                    }
                });

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
                        let color = match line.line_type {
                            ConsoleLineType::Normal => ui.style().visuals.text_color(),
                            ConsoleLineType::Command => egui::Color32::from_rgb(79, 195, 247),  // Cyan/blue
                            ConsoleLineType::Success => egui::Color32::from_rgb(102, 187, 106), // Green
                            ConsoleLineType::Warning => egui::Color32::from_rgb(255, 183, 77),  // Orange
                            ConsoleLineType::Error => egui::Color32::from_rgb(255, 84, 73),     // Red
                            ConsoleLineType::Summary => egui::Color32::from_rgb(149, 117, 205), // Purple
                        };
                        ui.colored_label(color, &line.text);
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

            // Dry Run mode - prominent option
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.options.dry_run.enabled, &self.options.dry_run.name);
                if self.options.dry_run.enabled {
                    ui.label(
                        egui::RichText::new("(Preview mode - no changes will be made)")
                            .small()
                            .color(egui::Color32::from_rgb(100, 180, 255)),
                    );
                }
            });
            ui.label(
                egui::RichText::new(&self.options.dry_run.description)
                    .small()
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);

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

        if let Some(error) = opt.validation_error() {
            ui.label(
                egui::RichText::new(format!("⚠ {}", error))
                    .small()
                    .color(egui::Color32::from_rgb(255, 180, 90)),
            );
        }

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
                            self.update_source_mode_from_path();
                        }

                        // Run button
                        if ui
                            .add_enabled(self.state == AppState::Idle, egui::Button::new("▶ Run"))
                            .clicked()
                        {
                            self.source_path = entry.source.clone();
                            self.destination_path = entry.destination.clone();
                            self.options = entry.options.clone();
                            self.update_source_mode_from_path();
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

    fn render_destination_hash_warning(&mut self, ctx: &egui::Context) {
        if !self.show_destination_hash_warning {
            return;
        }

        egui::Window::new("Destination Hash Verification")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("⚠ Performance Warning");
                ui.add_space(4.0);
                ui.label(
                    "Enabling destination hash verification will hash all files in the destination \
                    directory after the transfer completes. This can take considerable time if the \
                    destination is located on a slow network connection.\n\n\
                    If source hashing is also enabled, GRAFT will automatically compare source and \
                    destination hashes and report matched, mismatched, missing, and extra files.\n\n\
                    Typical performance:\n\
                    • Local drive: ~100-500 MB/s\n\
                    • LAN (Gigabit): ~50-100 MB/s\n\
                    • WAN/Slow network: 1-10 MB/s or slower\n\n\
                    For a 100 GB transfer on a slow WAN, hashing could take 2-3+ hours."
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.enable_destination_hashing = false;
                        self.show_destination_hash_warning = false;
                    }
                    if ui.button("✓ Continue").clicked() {
                        self.show_destination_hash_warning = false;
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
