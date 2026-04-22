//! Robocopy options and command building module

use serde::{Deserialize, Serialize};

/// A single Robocopy option with its flag and description
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RobocopyOption {
    pub flag: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub has_value: bool,
    pub value: String,
}

impl RobocopyOption {
    pub fn new(flag: &str, name: &str, description: &str) -> Self {
        Self {
            flag: flag.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            enabled: false,
            has_value: false,
            value: String::new(),
        }
    }

    pub fn with_value(flag: &str, name: &str, description: &str, default: &str) -> Self {
        Self {
            flag: flag.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            enabled: false,
            has_value: true,
            value: default.to_string(),
        }
    }

    /// Validate the value for this option when enabled.
    pub fn validation_error(&self) -> Option<String> {
        if !self.enabled || !self.has_value {
            return None;
        }

        let value = self.value.trim();

        if value.is_empty() {
            return Some("value cannot be empty".to_string());
        }

        if value.chars().any(|ch| ch.is_whitespace()) {
            return Some("value cannot contain spaces".to_string());
        }

        if value.contains('/') || value.contains('\\') {
            return Some("value cannot contain '/' or '\\'".to_string());
        }

        match self.flag.as_str() {
            "/LEV:" | "/R:" | "/W:" | "/IPG:" => {
                if !value.chars().all(|ch| ch.is_ascii_digit()) {
                    return Some("value must be a non-negative integer".to_string());
                }
            }
            "/MT:" => {
                if !value.chars().all(|ch| ch.is_ascii_digit()) {
                    return Some("value must be an integer between 1 and 128".to_string());
                }

                match value.parse::<u16>() {
                    Ok(v) if (1..=128).contains(&v) => {}
                    _ => return Some("value must be between 1 and 128".to_string()),
                }
            }
            "/A+:" | "/A-:" => {
                if !Self::contains_only_allowed_letters(value, "RASHCNET") {
                    return Some("value must use only attribute letters: R, A, S, H, C, N, E, T".to_string());
                }
            }
            "/COPY:" => {
                if !Self::contains_only_allowed_letters(value, "DATSOU") {
                    return Some("value must use only copy flags: D, A, T, S, O, U".to_string());
                }
            }
            "/DCOPY:" => {
                if !Self::contains_only_allowed_letters(value, "DATE") {
                    return Some("value must use only directory copy flags: D, A, T, E".to_string());
                }
            }
            _ => {}
        }

        None
    }

    fn contains_only_allowed_letters(value: &str, allowed: &str) -> bool {
        value
            .chars()
            .all(|ch| allowed.contains(ch.to_ascii_uppercase()))
    }
}

/// Preset groups for common operations
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PresetGroup {
    None,
    LargeFilesWan,
    MirrorWithMetadata,
    CopyAllPreserve,
    IncrementalBackup,
    QuickCopy,
}

impl PresetGroup {
    pub fn name(&self) -> &'static str {
        match self {
            PresetGroup::None => "Custom",
            PresetGroup::LargeFilesWan => "Large Files over WAN",
            PresetGroup::MirrorWithMetadata => "Mirror with Full Metadata",
            PresetGroup::CopyAllPreserve => "Copy All & Preserve Attributes",
            PresetGroup::IncrementalBackup => "Incremental Backup",
            PresetGroup::QuickCopy => "Quick Copy (No Extras)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            PresetGroup::None => "Manually select options below",
            PresetGroup::LargeFilesWan => "Optimized for copying large files over a WAN. Uses unbuffered I/O, conservative threading, and sensible retries.",
            PresetGroup::MirrorWithMetadata => "Creates an exact mirror copy including all file attributes, timestamps, security, and auditing info. Deletes files in destination not in source.",
            PresetGroup::CopyAllPreserve => "Copies all files preserving all metadata (timestamps, attributes, security, owner info). Does not delete destination files.",
            PresetGroup::IncrementalBackup => "Only copies new or changed files. Efficient for regular backups.",
            PresetGroup::QuickCopy => "Fast copy with minimal options. Good for simple file transfers.",
        }
    }
}

/// All Robocopy options organized by category
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RobocopyOptions {
    // Copy options
    pub copy_subdirs: RobocopyOption,
    pub copy_subdirs_empty: RobocopyOption,
    pub copy_levels: RobocopyOption,
    pub copy_restartable: RobocopyOption,
    pub copy_backup: RobocopyOption,
    pub copy_unbuffered: RobocopyOption,

    // File selection
    pub copy_all: RobocopyOption,
    pub copy_flags: RobocopyOption,
    pub dir_copy_flags: RobocopyOption,
    pub sec_copy: RobocopyOption,
    pub copy_timestamps: RobocopyOption,
    pub purge: RobocopyOption,
    pub mirror: RobocopyOption,
    pub move_files: RobocopyOption,
    pub move_files_dirs: RobocopyOption,

    // Attributes
    pub attr_add: RobocopyOption,
    pub attr_remove: RobocopyOption,
    pub create_tree: RobocopyOption,

    // Retry options
    pub retry_count: RobocopyOption,
    pub retry_wait: RobocopyOption,

    // Logging options
    pub log_verbose: RobocopyOption,
    pub log_timestamps: RobocopyOption,
    pub log_full_path: RobocopyOption,
    pub log_bytes: RobocopyOption,
    pub no_progress: RobocopyOption,
    pub log_eta: RobocopyOption,

    // File filter options
    pub exclude_changed: RobocopyOption,
    pub exclude_newer: RobocopyOption,
    pub exclude_older: RobocopyOption,
    pub exclude_extra: RobocopyOption,
    pub exclude_lonely: RobocopyOption,
    pub include_same: RobocopyOption,
    pub include_modified: RobocopyOption,

    // Performance
    pub multi_thread: RobocopyOption,
    pub inter_packet_gap: RobocopyOption,

    // Special modes
    pub dry_run: RobocopyOption,

    // Current preset
    pub current_preset: PresetGroup,
}

impl Default for RobocopyOptions {
    fn default() -> Self {
        const DEFAULT_PRESET: PresetGroup = PresetGroup::LargeFilesWan;
        
        let mut opts = Self {
            // Copy options
            copy_subdirs: RobocopyOption::new("/S", "Copy Subdirectories", "Copy subdirectories, excluding empty ones"),
            copy_subdirs_empty: RobocopyOption::new("/E", "Copy Empty Subdirs", "Copy subdirectories, including empty ones"),
            copy_levels: RobocopyOption::with_value("/LEV:", "Copy Levels", "Only copy the top N levels of the source directory tree", ""),
            copy_restartable: RobocopyOption::new("/Z", "Restartable Mode", "Copy files in restartable mode (survive network glitch)"),
            copy_backup: RobocopyOption::new("/B", "Backup Mode", "Copy files in backup mode (requires backup privileges)"),
            copy_unbuffered: RobocopyOption::new("/J", "Unbuffered I/O", "Copy using unbuffered I/O (recommended for large files)"),

            // File selection
            copy_all: RobocopyOption::new("/COPYALL", "Copy All Attributes", "Copy ALL file info (equivalent to /COPY:DATSOU)"),
            copy_flags: RobocopyOption::with_value("/COPY:", "Copy Flags", "What to COPY: D=Data, A=Attribs, T=Timestamps, S=Security, O=Owner, U=aUditing", "DAT"),
            dir_copy_flags: RobocopyOption::with_value("/DCOPY:", "Dir Copy Flags", "What to COPY for directories: D=Data, A=Attribs, T=Timestamps", "DA"),
            sec_copy: RobocopyOption::new("/SEC", "Copy Security", "Copy files with SECurity (equivalent to /COPY:DATS)"),
            copy_timestamps: RobocopyOption::new("/TIMFIX", "Fix Timestamps", "Fix file TIMes on all files, even skipped files"),
            purge: RobocopyOption::new("/PURGE", "Purge Destination", "Delete destination files/directories that no longer exist in source"),
            mirror: RobocopyOption::new("/MIR", "Mirror Mode", "Mirror a directory tree (equivalent to /E plus /PURGE)"),
            move_files: RobocopyOption::new("/MOV", "Move Files", "Move files (delete from source after copying)"),
            move_files_dirs: RobocopyOption::new("/MOVE", "Move Files & Dirs", "Move files and directories (delete from source after copying)"),

            // Attributes
            attr_add: RobocopyOption::with_value("/A+:", "Add Attributes", "Add the given attributes to copied files (R,A,S,H,C,N,E,T)", ""),
            attr_remove: RobocopyOption::with_value("/A-:", "Remove Attributes", "Remove the given attributes from copied files", ""),
            create_tree: RobocopyOption::new("/CREATE", "Create Tree Only", "Create directory tree and zero-length files only"),

            // Retry options
            retry_count: RobocopyOption::with_value("/R:", "Retry Count", "Number of retries on failed copies (default 1 million)", "3"),
            retry_wait: RobocopyOption::with_value("/W:", "Retry Wait", "Wait time between retries in seconds (default 30)", "5"),

            // Logging options
            log_verbose: RobocopyOption::new("/V", "Verbose Output", "Produce verbose output, showing skipped files"),
            log_timestamps: RobocopyOption::new("/TS", "Include Timestamps", "Include source file timestamps in the output"),
            log_full_path: RobocopyOption::new("/FP", "Full Pathnames", "Include full pathname of files in the output"),
            log_bytes: RobocopyOption::new("/BYTES", "Show Bytes", "Print sizes as bytes"),
            no_progress: RobocopyOption::new("/NP", "No Progress", "No progress - don't display percentage copied"),
            log_eta: RobocopyOption::new("/ETA", "Show ETA", "Show estimated time of arrival of copied files"),

            // File filter options
            exclude_changed: RobocopyOption::new("/XC", "Exclude Changed", "Exclude changed files"),
            exclude_newer: RobocopyOption::new("/XN", "Exclude Newer", "Exclude newer files"),
            exclude_older: RobocopyOption::new("/XO", "Exclude Older", "Exclude older files"),
            exclude_extra: RobocopyOption::new("/XX", "Exclude Extra", "Exclude extra files and directories (in destination, not in source)"),
            exclude_lonely: RobocopyOption::new("/XL", "Exclude Lonely", "Exclude lonely files and directories (in source, not in destination)"),
            include_same: RobocopyOption::new("/IS", "Include Same", "Include same files (overwrite even if identical)"),
            include_modified: RobocopyOption::new("/IT", "Include Tweaked", "Include tweaked files (same size, different timestamp)"),

            // Performance
            multi_thread: RobocopyOption::with_value("/MT:", "Multi-threaded", "Multi-threaded copy with N threads (default 8, max 128)", "8"),
            inter_packet_gap: RobocopyOption::with_value("/IPG:", "Inter-Packet Gap", "Inter-packet gap in milliseconds (for bandwidth throttling)", ""),

            // Special modes
            dry_run: RobocopyOption::new("/L", "Dry Run (List Only)", "List only - don't copy, delete or timestamp any files"),

            current_preset: DEFAULT_PRESET,
        };
        
        // Apply the default preset
        opts.apply_preset(DEFAULT_PRESET);
        opts
    }
}

impl RobocopyOptions {
    /// Validate all enabled value-bearing options and return user-facing errors.
    pub fn validate_enabled_options(&self) -> Vec<String> {
        let value_options = [
            &self.copy_levels,
            &self.copy_flags,
            &self.dir_copy_flags,
            &self.attr_add,
            &self.attr_remove,
            &self.retry_count,
            &self.retry_wait,
            &self.multi_thread,
            &self.inter_packet_gap,
        ];

        let mut errors = Vec::new();
        for option in value_options {
            if let Some(error) = option.validation_error() {
                errors.push(format!("{}: {}", option.name, error));
            }
        }

        errors
    }

    /// Create a new RobocopyOptions without applying any preset
    /// 
    /// This creates options in their default/disabled state without applying any preset.
    /// This is necessary to avoid infinite recursion when comparing current options
    /// against a preset in `matches_current_preset()`. Use `default()` for normal
    /// initialization, which applies the default preset.
    pub fn new_empty() -> Self {
        Self {
            // Copy options
            copy_subdirs: RobocopyOption::new("/S", "Copy Subdirectories", "Copy subdirectories, excluding empty ones"),
            copy_subdirs_empty: RobocopyOption::new("/E", "Copy Empty Subdirs", "Copy subdirectories, including empty ones"),
            copy_levels: RobocopyOption::with_value("/LEV:", "Copy Levels", "Only copy the top N levels of the source directory tree", ""),
            copy_restartable: RobocopyOption::new("/Z", "Restartable Mode", "Copy files in restartable mode (survive network glitch)"),
            copy_backup: RobocopyOption::new("/B", "Backup Mode", "Copy files in backup mode (requires backup privileges)"),
            copy_unbuffered: RobocopyOption::new("/J", "Unbuffered I/O", "Copy using unbuffered I/O (recommended for large files)"),

            // File selection
            copy_all: RobocopyOption::new("/COPYALL", "Copy All Attributes", "Copy ALL file info (equivalent to /COPY:DATSOU)"),
            copy_flags: RobocopyOption::with_value("/COPY:", "Copy Flags", "What to COPY: D=Data, A=Attribs, T=Timestamps, S=Security, O=Owner, U=aUditing", "DAT"),
            dir_copy_flags: RobocopyOption::with_value("/DCOPY:", "Dir Copy Flags", "What to COPY for directories: D=Data, A=Attribs, T=Timestamps", "DA"),
            sec_copy: RobocopyOption::new("/SEC", "Copy Security", "Copy files with SECurity (equivalent to /COPY:DATS)"),
            copy_timestamps: RobocopyOption::new("/TIMFIX", "Fix Timestamps", "Fix file TIMes on all files, even skipped files"),
            purge: RobocopyOption::new("/PURGE", "Purge Destination", "Delete destination files/directories that no longer exist in source"),
            mirror: RobocopyOption::new("/MIR", "Mirror Mode", "Mirror a directory tree (equivalent to /E plus /PURGE)"),
            move_files: RobocopyOption::new("/MOV", "Move Files", "Move files (delete from source after copying)"),
            move_files_dirs: RobocopyOption::new("/MOVE", "Move Files & Dirs", "Move files and directories (delete from source after copying)"),

            // Attributes
            attr_add: RobocopyOption::with_value("/A+:", "Add Attributes", "Add the given attributes to copied files (R,A,S,H,C,N,E,T)", ""),
            attr_remove: RobocopyOption::with_value("/A-:", "Remove Attributes", "Remove the given attributes from copied files", ""),
            create_tree: RobocopyOption::new("/CREATE", "Create Tree Only", "Create directory tree and zero-length files only"),

            // Retry options
            retry_count: RobocopyOption::with_value("/R:", "Retry Count", "Number of retries on failed copies (default 1 million)", "3"),
            retry_wait: RobocopyOption::with_value("/W:", "Retry Wait", "Wait time between retries in seconds (default 30)", "5"),

            // Logging options
            log_verbose: RobocopyOption::new("/V", "Verbose Output", "Produce verbose output, showing skipped files"),
            log_timestamps: RobocopyOption::new("/TS", "Include Timestamps", "Include source file timestamps in the output"),
            log_full_path: RobocopyOption::new("/FP", "Full Pathnames", "Include full pathname of files in the output"),
            log_bytes: RobocopyOption::new("/BYTES", "Show Bytes", "Print sizes as bytes"),
            no_progress: RobocopyOption::new("/NP", "No Progress", "No progress - don't display percentage copied"),
            log_eta: RobocopyOption::new("/ETA", "Show ETA", "Show estimated time of arrival of copied files"),

            // File filter options
            exclude_changed: RobocopyOption::new("/XC", "Exclude Changed", "Exclude changed files"),
            exclude_newer: RobocopyOption::new("/XN", "Exclude Newer", "Exclude newer files"),
            exclude_older: RobocopyOption::new("/XO", "Exclude Older", "Exclude older files"),
            exclude_extra: RobocopyOption::new("/XX", "Exclude Extra", "Exclude extra files and directories (in destination, not in source)"),
            exclude_lonely: RobocopyOption::new("/XL", "Exclude Lonely", "Exclude lonely files and directories (in source, not in destination)"),
            include_same: RobocopyOption::new("/IS", "Include Same", "Include same files (overwrite even if identical)"),
            include_modified: RobocopyOption::new("/IT", "Include Tweaked", "Include tweaked files (same size, different timestamp)"),

            // Performance
            multi_thread: RobocopyOption::with_value("/MT:", "Multi-threaded", "Multi-threaded copy with N threads (default 8, max 128)", "8"),
            inter_packet_gap: RobocopyOption::with_value("/IPG:", "Inter-Packet Gap", "Inter-packet gap in milliseconds (for bandwidth throttling)", ""),

            // Special modes
            dry_run: RobocopyOption::new("/L", "Dry Run (List Only)", "List only - don't copy, delete or timestamp any files"),

            current_preset: PresetGroup::None,
        }
    }

    /// Apply a preset configuration
    pub fn apply_preset(&mut self, preset: PresetGroup) {
        // Reset all options first
        self.reset_all();
        self.current_preset = preset.clone();

        match preset {
            PresetGroup::None => {}
            PresetGroup::LargeFilesWan => {
                self.copy_subdirs_empty.enabled = true;
                self.copy_flags.enabled = true;
                self.copy_flags.value = "DAT".to_string();
                self.dir_copy_flags.enabled = true;
                self.dir_copy_flags.value = "DAT".to_string();
                self.copy_unbuffered.enabled = true;
                self.no_progress.enabled = true;
                self.retry_count.enabled = true;
                self.retry_count.value = "3".to_string();
                self.retry_wait.enabled = true;
                self.retry_wait.value = "5".to_string();
                self.multi_thread.enabled = true;
                self.multi_thread.value = "8".to_string();
            }
            PresetGroup::MirrorWithMetadata => {
                self.mirror.enabled = true;
                // Use /COPY:DATS instead of /COPYALL to avoid requiring "Manage Auditing" privilege
                self.copy_flags.enabled = true;
                self.copy_flags.value = "DATS".to_string();
                self.copy_restartable.enabled = true;
                self.retry_count.enabled = true;
                self.retry_count.value = "3".to_string();
                self.retry_wait.enabled = true;
                self.retry_wait.value = "5".to_string();
                self.multi_thread.enabled = true;
                self.multi_thread.value = "8".to_string();
            }
            PresetGroup::CopyAllPreserve => {
                self.copy_subdirs_empty.enabled = true;
                // Use /COPY:DATS instead of /COPYALL to avoid requiring "Manage Auditing" privilege
                // DATS = Data, Attributes, Timestamps, Security (no Owner or aUditing)
                self.copy_flags.enabled = true;
                self.copy_flags.value = "DATS".to_string();
                self.copy_restartable.enabled = true;
                self.retry_count.enabled = true;
                self.retry_count.value = "3".to_string();
                self.retry_wait.enabled = true;
                self.retry_wait.value = "5".to_string();
                self.multi_thread.enabled = true;
                self.multi_thread.value = "8".to_string();
            }
            PresetGroup::IncrementalBackup => {
                self.copy_subdirs_empty.enabled = true;
                self.copy_flags.enabled = true;
                self.copy_flags.value = "DAT".to_string();
                self.exclude_older.enabled = true;
                self.retry_count.enabled = true;
                self.retry_count.value = "3".to_string();
                self.retry_wait.enabled = true;
                self.retry_wait.value = "5".to_string();
                self.multi_thread.enabled = true;
                self.multi_thread.value = "8".to_string();
            }
            PresetGroup::QuickCopy => {
                self.copy_subdirs_empty.enabled = true;
                self.multi_thread.enabled = true;
                self.multi_thread.value = "16".to_string();
                self.retry_count.enabled = true;
                self.retry_count.value = "1".to_string();
                self.retry_wait.enabled = true;
                self.retry_wait.value = "1".to_string();
            }
        }
    }

    /// Check if current options match the current preset
    pub fn matches_current_preset(&self) -> bool {
        if self.current_preset == PresetGroup::None {
            return true; // Custom always matches itself
        }

        // Create a temporary options object with the preset applied
        let mut temp = Self::new_empty();
        temp.apply_preset(self.current_preset.clone());

        // Compare all relevant option states
        self.options_equal(&temp)
    }

    /// Helper to check if two RobocopyOptions have the same enabled options and values
    fn options_equal(&self, other: &Self) -> bool {
        // Compare all options
        self.copy_subdirs.enabled == other.copy_subdirs.enabled &&
        self.copy_subdirs_empty.enabled == other.copy_subdirs_empty.enabled &&
        self.copy_levels.enabled == other.copy_levels.enabled &&
        self.copy_levels.value == other.copy_levels.value &&
        self.copy_restartable.enabled == other.copy_restartable.enabled &&
        self.copy_backup.enabled == other.copy_backup.enabled &&
        self.copy_unbuffered.enabled == other.copy_unbuffered.enabled &&
        
        self.copy_all.enabled == other.copy_all.enabled &&
        self.copy_flags.enabled == other.copy_flags.enabled &&
        self.copy_flags.value == other.copy_flags.value &&
        self.dir_copy_flags.enabled == other.dir_copy_flags.enabled &&
        self.dir_copy_flags.value == other.dir_copy_flags.value &&
        self.sec_copy.enabled == other.sec_copy.enabled &&
        self.copy_timestamps.enabled == other.copy_timestamps.enabled &&
        self.purge.enabled == other.purge.enabled &&
        self.mirror.enabled == other.mirror.enabled &&
        self.move_files.enabled == other.move_files.enabled &&
        self.move_files_dirs.enabled == other.move_files_dirs.enabled &&
        
        self.attr_add.enabled == other.attr_add.enabled &&
        self.attr_add.value == other.attr_add.value &&
        self.attr_remove.enabled == other.attr_remove.enabled &&
        self.attr_remove.value == other.attr_remove.value &&
        self.create_tree.enabled == other.create_tree.enabled &&
        
        self.retry_count.enabled == other.retry_count.enabled &&
        self.retry_count.value == other.retry_count.value &&
        self.retry_wait.enabled == other.retry_wait.enabled &&
        self.retry_wait.value == other.retry_wait.value &&
        
        self.log_verbose.enabled == other.log_verbose.enabled &&
        self.log_timestamps.enabled == other.log_timestamps.enabled &&
        self.log_full_path.enabled == other.log_full_path.enabled &&
        self.log_bytes.enabled == other.log_bytes.enabled &&
        self.no_progress.enabled == other.no_progress.enabled &&
        self.log_eta.enabled == other.log_eta.enabled &&
        
        self.exclude_changed.enabled == other.exclude_changed.enabled &&
        self.exclude_newer.enabled == other.exclude_newer.enabled &&
        self.exclude_older.enabled == other.exclude_older.enabled &&
        self.exclude_extra.enabled == other.exclude_extra.enabled &&
        self.exclude_lonely.enabled == other.exclude_lonely.enabled &&
        self.include_same.enabled == other.include_same.enabled &&
        self.include_modified.enabled == other.include_modified.enabled &&
        
        self.multi_thread.enabled == other.multi_thread.enabled &&
        self.multi_thread.value == other.multi_thread.value &&
        self.inter_packet_gap.enabled == other.inter_packet_gap.enabled &&
        self.inter_packet_gap.value == other.inter_packet_gap.value &&
        
        self.dry_run.enabled == other.dry_run.enabled
    }

    /// Reset all options to disabled
    pub fn reset_all(&mut self) {
        let default = Self::new_empty();
        
        // Copy options
        self.copy_subdirs.enabled = false;
        self.copy_subdirs_empty.enabled = false;
        self.copy_levels.enabled = false;
        self.copy_levels.value = default.copy_levels.value;
        self.copy_restartable.enabled = false;
        self.copy_backup.enabled = false;
        self.copy_unbuffered.enabled = false;

        // File selection
        self.copy_all.enabled = false;
        self.copy_flags.enabled = false;
        self.copy_flags.value = default.copy_flags.value;
        self.dir_copy_flags.enabled = false;
        self.dir_copy_flags.value = default.dir_copy_flags.value;
        self.sec_copy.enabled = false;
        self.copy_timestamps.enabled = false;
        self.purge.enabled = false;
        self.mirror.enabled = false;
        self.move_files.enabled = false;
        self.move_files_dirs.enabled = false;

        // Attributes
        self.attr_add.enabled = false;
        self.attr_add.value = default.attr_add.value;
        self.attr_remove.enabled = false;
        self.attr_remove.value = default.attr_remove.value;
        self.create_tree.enabled = false;

        // Retry options
        self.retry_count.enabled = false;
        self.retry_count.value = default.retry_count.value;
        self.retry_wait.enabled = false;
        self.retry_wait.value = default.retry_wait.value;

        // Logging options
        self.log_verbose.enabled = false;
        self.log_timestamps.enabled = false;
        self.log_full_path.enabled = false;
        self.log_bytes.enabled = false;
        self.no_progress.enabled = false;
        self.log_eta.enabled = false;

        // File filter options
        self.exclude_changed.enabled = false;
        self.exclude_newer.enabled = false;
        self.exclude_older.enabled = false;
        self.exclude_extra.enabled = false;
        self.exclude_lonely.enabled = false;
        self.include_same.enabled = false;
        self.include_modified.enabled = false;

        // Performance
        self.multi_thread.enabled = false;
        self.multi_thread.value = default.multi_thread.value;
        self.inter_packet_gap.enabled = false;
        self.inter_packet_gap.value = default.inter_packet_gap.value;

        // Special modes
        self.dry_run.enabled = false;
    }

    /// Build the command line arguments
    pub fn build_args(&self, source: &str, destination: &str) -> Vec<String> {
        let mut args = vec![
            source.to_string(),
            destination.to_string(),
        ];

        // Helper to add option
        let add_opt = |args: &mut Vec<String>, opt: &RobocopyOption| {
            if opt.enabled {
                if opt.has_value && !opt.value.is_empty() {
                    args.push(format!("{}{}", opt.flag, opt.value));
                } else if !opt.has_value {
                    args.push(opt.flag.clone());
                }
            }
        };

        // Copy options
        add_opt(&mut args, &self.copy_subdirs);
        add_opt(&mut args, &self.copy_subdirs_empty);
        add_opt(&mut args, &self.copy_levels);
        add_opt(&mut args, &self.copy_restartable);
        add_opt(&mut args, &self.copy_backup);
        add_opt(&mut args, &self.copy_unbuffered);

        // File selection
        add_opt(&mut args, &self.copy_all);
        add_opt(&mut args, &self.copy_flags);
        add_opt(&mut args, &self.dir_copy_flags);
        add_opt(&mut args, &self.sec_copy);
        add_opt(&mut args, &self.copy_timestamps);
        add_opt(&mut args, &self.purge);
        add_opt(&mut args, &self.mirror);
        add_opt(&mut args, &self.move_files);
        add_opt(&mut args, &self.move_files_dirs);

        // Attributes
        add_opt(&mut args, &self.attr_add);
        add_opt(&mut args, &self.attr_remove);
        add_opt(&mut args, &self.create_tree);

        // Retry options
        add_opt(&mut args, &self.retry_count);
        add_opt(&mut args, &self.retry_wait);

        // Logging options
        add_opt(&mut args, &self.log_verbose);
        add_opt(&mut args, &self.log_timestamps);
        add_opt(&mut args, &self.log_full_path);
        add_opt(&mut args, &self.log_bytes);
        add_opt(&mut args, &self.no_progress);
        add_opt(&mut args, &self.log_eta);

        // File filter options
        add_opt(&mut args, &self.exclude_changed);
        add_opt(&mut args, &self.exclude_newer);
        add_opt(&mut args, &self.exclude_older);
        add_opt(&mut args, &self.exclude_extra);
        add_opt(&mut args, &self.exclude_lonely);
        add_opt(&mut args, &self.include_same);
        add_opt(&mut args, &self.include_modified);

        // Performance
        add_opt(&mut args, &self.multi_thread);
        add_opt(&mut args, &self.inter_packet_gap);

        // Special modes
        add_opt(&mut args, &self.dry_run);

        args
    }

    /// Get the full command string for display
    pub fn build_command_string(&self, source: &str, destination: &str) -> String {
        let args = self.build_args(source, destination);
        let display_args: Vec<String> = args
            .iter()
            .map(|arg| Self::quote_arg_for_display(arg))
            .collect();
        format!("robocopy {}", display_args.join(" "))
    }

    fn quote_arg_for_display(arg: &str) -> String {
        let escaped = arg.replace('"', "\\\"");
        if arg.is_empty() || arg.chars().any(|ch| ch.is_whitespace()) {
            format!("\"{}\"", escaped)
        } else {
            escaped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let options = RobocopyOptions::default();
        // Default preset is now LargeFilesWan
        assert_eq!(options.current_preset, PresetGroup::LargeFilesWan);
        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.copy_unbuffered.enabled);
    }

    #[test]
    fn test_basic_command_building() {
        // Create empty options for basic command test
        let mut options = RobocopyOptions::new_empty();
        options.current_preset = PresetGroup::None;
        let cmd = options.build_command_string("C:\\source", "C:\\dest");
        assert_eq!(cmd, "robocopy C:\\source C:\\dest");
    }

    #[test]
    fn test_mirror_preset() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::MirrorWithMetadata);
        
        assert!(options.mirror.enabled);
        assert!(options.copy_flags.enabled);
        assert_eq!(options.copy_flags.value, "DATS");
        assert!(options.copy_restartable.enabled);
        assert!(options.retry_count.enabled);
        assert_eq!(options.retry_count.value, "3");
        assert!(options.multi_thread.enabled);
        assert_eq!(options.multi_thread.value, "8");
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/MIR".to_string()));
        assert!(args.contains(&"/COPY:DATS".to_string()));
        assert!(args.contains(&"/Z".to_string()));
        assert!(args.contains(&"/R:3".to_string()));
        assert!(args.contains(&"/MT:8".to_string()));
    }

    #[test]
    fn test_copy_all_preserve_preset() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::CopyAllPreserve);
        
        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.copy_flags.enabled);
        assert_eq!(options.copy_flags.value, "DATS");
        assert!(!options.copy_all.enabled, "Should use COPY:DATS instead of COPYALL to avoid admin requirement");
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/E".to_string()));
        assert!(args.contains(&"/COPY:DATS".to_string()));
        assert!(!args.iter().any(|a| a.contains("COPYALL")));
    }

    #[test]
    fn test_incremental_backup_preset() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::IncrementalBackup);
        
        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.exclude_older.enabled);
        assert!(options.multi_thread.enabled);
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/E".to_string()));
        assert!(args.contains(&"/XO".to_string()));
    }

    #[test]
    fn test_quick_copy_preset() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::QuickCopy);
        
        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.multi_thread.enabled);
        assert_eq!(options.multi_thread.value, "16");
        assert_eq!(options.retry_count.value, "1");
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/MT:16".to_string()));
        assert!(args.contains(&"/R:1".to_string()));
    }

    #[test]
    fn test_reset_all() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::MirrorWithMetadata);
        options.reset_all();
        
        assert!(!options.mirror.enabled);
        assert!(!options.copy_all.enabled);
        assert!(!options.multi_thread.enabled);
    }

    #[test]
    fn test_option_with_value() {
        let mut options = RobocopyOptions::default();
        options.multi_thread.enabled = true;
        options.multi_thread.value = "16".to_string();
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/MT:16".to_string()));
    }

    #[test]
    fn test_option_with_empty_value_not_added() {
        let mut options = RobocopyOptions::default();
        options.copy_levels.enabled = true;
        options.copy_levels.value = "".to_string();
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(!args.iter().any(|a| a.contains("/LEV:")));
    }

    #[test]
    fn test_multiple_options() {
        let mut options = RobocopyOptions::default();
        options.copy_subdirs_empty.enabled = true;
        options.copy_restartable.enabled = true;
        options.log_verbose.enabled = true;
        options.no_progress.enabled = true;
        
        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/E".to_string()));
        assert!(args.contains(&"/Z".to_string()));
        assert!(args.contains(&"/V".to_string()));
        assert!(args.contains(&"/NP".to_string()));
    }

    #[test]
    fn test_large_files_wan_preset() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::LargeFilesWan);

        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.copy_flags.enabled);
        assert_eq!(options.copy_flags.value, "DAT");
        assert!(options.dir_copy_flags.enabled);
        assert_eq!(options.dir_copy_flags.value, "DAT");
        assert!(options.copy_unbuffered.enabled);
        assert!(options.no_progress.enabled);
        assert!(options.retry_count.enabled);
        assert_eq!(options.retry_count.value, "3");
        assert!(options.retry_wait.enabled);
        assert_eq!(options.retry_wait.value, "5");
        assert!(options.multi_thread.enabled);
        assert_eq!(options.multi_thread.value, "8");
        assert!(!options.copy_restartable.enabled, "/J and /Z are mutually exclusive");

        let args = options.build_args("C:\\src", "C:\\dst");
        assert!(args.contains(&"/E".to_string()));
        assert!(args.contains(&"/COPY:DAT".to_string()));
        assert!(args.contains(&"/DCOPY:DAT".to_string()));
        assert!(args.contains(&"/J".to_string()));
        assert!(args.contains(&"/NP".to_string()));
        assert!(args.contains(&"/R:3".to_string()));
        assert!(args.contains(&"/W:5".to_string()));
        assert!(args.contains(&"/MT:8".to_string()));
        assert!(!args.contains(&"/Z".to_string()));
    }

    #[test]
    fn test_preset_names() {
        assert_eq!(PresetGroup::None.name(), "Custom");
        assert_eq!(PresetGroup::LargeFilesWan.name(), "Large Files over WAN");
        assert_eq!(PresetGroup::MirrorWithMetadata.name(), "Mirror with Full Metadata");
        assert_eq!(PresetGroup::CopyAllPreserve.name(), "Copy All & Preserve Attributes");
        assert_eq!(PresetGroup::IncrementalBackup.name(), "Incremental Backup");
        assert_eq!(PresetGroup::QuickCopy.name(), "Quick Copy (No Extras)");
    }

    #[test]
    fn test_robocopy_option_new() {
        let opt = RobocopyOption::new("/TEST", "Test Option", "A test description");
        assert_eq!(opt.flag, "/TEST");
        assert_eq!(opt.name, "Test Option");
        assert!(!opt.enabled);
        assert!(!opt.has_value);
    }

    #[test]
    fn test_robocopy_option_with_value() {
        let opt = RobocopyOption::with_value("/TEST:", "Test", "Description", "default");
        assert!(opt.has_value);
        assert_eq!(opt.value, "default");
    }

    #[test]
    fn test_preset_descriptions() {
        // Verify all presets have descriptions
        assert!(!PresetGroup::None.description().is_empty());
        assert!(!PresetGroup::LargeFilesWan.description().is_empty());
        assert!(!PresetGroup::MirrorWithMetadata.description().is_empty());
        assert!(!PresetGroup::CopyAllPreserve.description().is_empty());
        assert!(!PresetGroup::IncrementalBackup.description().is_empty());
        assert!(!PresetGroup::QuickCopy.description().is_empty());
        
        // Verify descriptions are meaningful
        assert!(PresetGroup::LargeFilesWan.description().contains("WAN"));
        assert!(PresetGroup::MirrorWithMetadata.description().contains("mirror"));
        assert!(PresetGroup::IncrementalBackup.description().contains("new or changed"));
    }

    #[test]
    fn test_matches_current_preset_true() {
        let mut options = RobocopyOptions::default();
        
        // Default preset is LargeFilesWan, so it should match
        assert!(options.matches_current_preset());
        
        // Apply a different preset and verify it matches
        options.apply_preset(PresetGroup::QuickCopy);
        assert!(options.matches_current_preset());
    }

    #[test]
    fn test_matches_current_preset_false_after_modification() {
        let mut options = RobocopyOptions::default();
        options.apply_preset(PresetGroup::QuickCopy);
        
        // Verify it matches initially
        assert!(options.matches_current_preset());
        
        // Modify an option
        options.copy_backup.enabled = true;
        
        // Now it should not match
        assert!(!options.matches_current_preset());
    }

    #[test]
    fn test_matches_current_preset_custom_always_matches() {
        let mut options = RobocopyOptions::new_empty();
        options.current_preset = PresetGroup::None;
        
        // Custom preset always matches
        assert!(options.matches_current_preset());
        
        // Even after modifications
        options.mirror.enabled = true;
        assert!(options.matches_current_preset());
    }

    #[test]
    fn test_options_equal() {
        let options1 = RobocopyOptions::default();
        let options2 = RobocopyOptions::default();
        
        assert!(options1.options_equal(&options2));
    }

    #[test]
    fn test_options_not_equal_different_enabled() {
        let mut options1 = RobocopyOptions::default();
        let options2 = RobocopyOptions::default();
        
        options1.copy_backup.enabled = true;
        
        assert!(!options1.options_equal(&options2));
    }

    #[test]
    fn test_options_not_equal_different_values() {
        let mut options1 = RobocopyOptions::default();
        let mut options2 = RobocopyOptions::default();
        
        options1.multi_thread.enabled = true;
        options1.multi_thread.value = "16".to_string();
        
        options2.multi_thread.enabled = true;
        options2.multi_thread.value = "8".to_string();
        
        assert!(!options1.options_equal(&options2));
    }

    #[test]
    fn test_build_command_string() {
        let mut options = RobocopyOptions::new_empty();
        options.copy_subdirs_empty.enabled = true;
        options.mirror.enabled = true;
        
        let cmd = options.build_command_string("C:\\source", "D:\\destination");
        
        assert!(cmd.starts_with("robocopy"));
        assert!(cmd.contains("C:\\source"));
        assert!(cmd.contains("D:\\destination"));
        assert!(cmd.contains("/E"));
        assert!(cmd.contains("/MIR"));
    }

    #[test]
    fn test_build_command_string_with_quoted_paths() {
        let options = RobocopyOptions::new_empty();
        
        let cmd = options.build_command_string("C:\\path with spaces", "D:\\another path");
        
        assert!(cmd.contains("\"C:\\path with spaces\""));
        assert!(cmd.contains("\"D:\\another path\""));
    }

    #[test]
    fn test_build_command_string_with_values() {
        let mut options = RobocopyOptions::new_empty();
        options.retry_count.enabled = true;
        options.retry_count.value = "5".to_string();
        options.multi_thread.enabled = true;
        options.multi_thread.value = "16".to_string();
        
        let cmd = options.build_command_string("C:\\src", "D:\\dst");
        
        assert!(cmd.contains("/R:5"));
        assert!(cmd.contains("/MT:16"));
    }

    #[test]
    fn test_new_empty_creates_disabled_options() {
        let options = RobocopyOptions::new_empty();
        
        assert!(!options.copy_subdirs.enabled);
        assert!(!options.copy_subdirs_empty.enabled);
        assert!(!options.mirror.enabled);
        assert!(!options.copy_all.enabled);
        assert!(!options.multi_thread.enabled);
        assert_eq!(options.current_preset, PresetGroup::None);
    }

    #[test]
    fn test_default_applies_preset() {
        let options = RobocopyOptions::default();
        
        // Default should apply LargeFilesWan preset
        assert_eq!(options.current_preset, PresetGroup::LargeFilesWan);
        assert!(options.copy_subdirs_empty.enabled);
        assert!(options.copy_unbuffered.enabled);
    }

    #[test]
    fn test_preset_application_resets_previous_options() {
        let mut options = RobocopyOptions::default();
        
        // Apply one preset
        options.apply_preset(PresetGroup::MirrorWithMetadata);
        assert!(options.mirror.enabled);
        
        // Apply another preset - should reset previous options
        options.apply_preset(PresetGroup::QuickCopy);
        assert!(!options.mirror.enabled);
        assert!(options.copy_subdirs_empty.enabled);
    }

    #[test]
    fn test_all_presets_produce_valid_commands() {
        let presets = vec![
            PresetGroup::None,
            PresetGroup::LargeFilesWan,
            PresetGroup::MirrorWithMetadata,
            PresetGroup::CopyAllPreserve,
            PresetGroup::IncrementalBackup,
            PresetGroup::QuickCopy,
        ];
        
        for preset in presets {
            let mut options = RobocopyOptions::default();
            options.apply_preset(preset.clone());
            
            let cmd = options.build_command_string("C:\\src", "D:\\dst");
            
            // Should always start with robocopy
            assert!(cmd.starts_with("robocopy"));
            
            // Should contain source and destination
            assert!(cmd.contains("C:\\src"));
            assert!(cmd.contains("D:\\dst"));
        }
    }

    #[test]
    fn test_option_flag_format() {
        let opt1 = RobocopyOption::new("/TEST", "Test", "Description");
        assert!(opt1.flag.starts_with('/'));
        
        let opt2 = RobocopyOption::with_value("/VAL:", "Value", "Description", "default");
        assert!(opt2.flag.ends_with(':'));
    }

    #[test]
    fn test_build_args_order_is_consistent() {
        let mut options = RobocopyOptions::new_empty();
        options.copy_subdirs_empty.enabled = true;
        options.retry_count.enabled = true;
        options.retry_count.value = "3".to_string();
        
        let args1 = options.build_args("C:\\src", "D:\\dst");
        let args2 = options.build_args("C:\\src", "D:\\dst");
        
        // Should produce identical args
        assert_eq!(args1, args2);
    }

    #[test]
    fn test_attribute_options() {
        let mut options = RobocopyOptions::new_empty();
        options.attr_add.enabled = true;
        options.attr_add.value = "RH".to_string();
        options.attr_remove.enabled = true;
        options.attr_remove.value = "A".to_string();
        
        let args = options.build_args("C:\\src", "D:\\dst");
        
        assert!(args.contains(&"/A+:RH".to_string()));
        assert!(args.contains(&"/A-:A".to_string()));
    }

    #[test]
    fn test_dry_run_mode() {
        let mut options = RobocopyOptions::new_empty();
        options.dry_run.enabled = true;
        
        let args = options.build_args("C:\\src", "D:\\dst");
        
        assert!(args.contains(&"/L".to_string()));
        
        let cmd = options.build_command_string("C:\\src", "D:\\dst");
        assert!(cmd.contains("/L"));
    }

    #[test]
    fn test_validate_enabled_options_rejects_space() {
        let mut options = RobocopyOptions::new_empty();
        options.retry_count.enabled = true;
        options.retry_count.value = "1 0".to_string();

        let errors = options.validate_enabled_options();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Retry Count"));
    }

    #[test]
    fn test_validate_enabled_options_rejects_forward_slash() {
        let mut options = RobocopyOptions::new_empty();
        options.retry_wait.enabled = true;
        options.retry_wait.value = "5/0".to_string();

        let errors = options.validate_enabled_options();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cannot contain '/' or '\\\\'"));
    }

    #[test]
    fn test_validate_enabled_options_rejects_backslash() {
        let mut options = RobocopyOptions::new_empty();
        options.retry_wait.enabled = true;
        options.retry_wait.value = "5\\0".to_string();

        let errors = options.validate_enabled_options();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cannot contain '/' or '\\\\'"));
    }

    #[test]
    fn test_validate_enabled_options_rejects_invalid_copy_flags() {
        let mut options = RobocopyOptions::new_empty();
        options.copy_flags.enabled = true;
        options.copy_flags.value = "DZX".to_string();

        let errors = options.validate_enabled_options();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("copy flags"));
    }

    #[test]
    fn test_validate_enabled_options_rejects_multi_thread_range() {
        let mut options = RobocopyOptions::new_empty();
        options.multi_thread.enabled = true;
        options.multi_thread.value = "129".to_string();

        let errors = options.validate_enabled_options();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("between 1 and 128"));
    }

    #[test]
    fn test_validate_enabled_options_accepts_valid_values() {
        let mut options = RobocopyOptions::new_empty();
        options.retry_count.enabled = true;
        options.retry_count.value = "3".to_string();
        options.copy_flags.enabled = true;
        options.copy_flags.value = "dats".to_string();
        options.dir_copy_flags.enabled = true;
        options.dir_copy_flags.value = "date".to_string();
        options.attr_add.enabled = true;
        options.attr_add.value = "RA".to_string();
        options.multi_thread.enabled = true;
        options.multi_thread.value = "16".to_string();

        let errors = options.validate_enabled_options();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_build_command_string_quotes_option_arg_with_spaces() {
        let mut options = RobocopyOptions::new_empty();
        options.copy_flags.enabled = true;
        options.copy_flags.value = "DAT SOU".to_string();

        let cmd = options.build_command_string("C:\\src", "D:\\dst");
        assert!(cmd.contains("\"/COPY:DAT SOU\""));
    }

    #[test]
    fn test_build_args_keeps_execution_args_unquoted() {
        let mut options = RobocopyOptions::new_empty();
        options.copy_flags.enabled = true;
        options.copy_flags.value = "DATSOU".to_string();

        let args = options.build_args("C:\\src path", "D:\\dst path");
        assert_eq!(args[0], "C:\\src path");
        assert_eq!(args[1], "D:\\dst path");
        assert!(args.contains(&"/COPY:DATSOU".to_string()));
    }
}
