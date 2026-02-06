#![windows_subsystem = "windows"]

mod app;
mod hasher;
mod history;
mod robocopy;

use app::GraftApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let icon = create_icon();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([1000.0, 700.0])
            .with_title("GRAFT - Graphical Robocopy Assured File Transfer Tool")
            .with_icon(icon),
        ..Default::default()
    };
    
    eframe::run_native(
        "Graft",
        options,
        Box::new(|cc| Ok(Box::new(GraftApp::new(cc)))),
    )
}

/// Create the application icon (stylized G) for the window/taskbar
fn create_icon() -> egui::IconData {
    let size = 64u32;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    
    // Colors (Material Design inspired)
    let bg_color = [0x1C, 0x1B, 0x1F, 0xFF];        // Dark surface
    let g_color = [0x4F, 0xC3, 0xF7, 0xFF];         // Primary cyan/teal
    let highlight = [0x80, 0xCB, 0xC4, 0xFF];       // Secondary teal
    
    let s = size as f32;
    let center_x = s / 2.0;
    let center_y = s / 2.0;
    
    // Fill background
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            pixels[idx..idx+4].copy_from_slice(&bg_color);
        }
    }
    
    // Draw a stylized "G"
    let stroke_width = s * 0.11;
    let radius = s * 0.34;
    
    // Gap angle: the opening of the C faces right
    let gap_start = -std::f32::consts::PI * 0.28;
    let gap_end = std::f32::consts::PI * 0.28;
    
    // Crossbar parameters
    let bar_y_center = center_y;
    let bar_left = center_x - s * 0.02;
    let bar_right = center_x + radius * gap_end.cos() + stroke_width * 0.5;
    
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32;
            let fy = y as f32;
            
            let mut draw = false;
            let mut use_highlight = false;
            
            // Main arc of the G
            let dx = fx - center_x;
            let dy = fy - center_y;
            let dist = (dx * dx + dy * dy).sqrt();
            
            if (dist - radius).abs() < stroke_width {
                let angle = dy.atan2(dx);
                // Draw the arc everywhere except the gap
                if angle < gap_start || angle > gap_end {
                    draw = true;
                    // Highlight the top portion
                    if angle < -std::f32::consts::PI * 0.4 && angle > -std::f32::consts::PI * 0.9 {
                        use_highlight = true;
                    }
                }
            }
            
            // Horizontal crossbar
            if (fy - bar_y_center).abs() < stroke_width
                && fx >= bar_left && fx <= bar_right {
                draw = true;
                use_highlight = true;
            }
            
            // Small vertical serif at the end of the top arc opening
            let top_end_x = center_x + radius * gap_start.cos();
            let top_end_y = center_y + radius * gap_start.sin();
            let serif_len = stroke_width * 1.2;
            if (fx - top_end_x).abs() < stroke_width
                && fy >= top_end_y - serif_len * 0.3 && fy <= top_end_y + serif_len {
                draw = true;
            }
            
            if draw {
                let idx = ((y * size + x) * 4) as usize;
                let color = if use_highlight { highlight } else { g_color };
                pixels[idx..idx+4].copy_from_slice(&color);
            }
        }
    }
    
    egui::IconData {
        rgba: pixels,
        width: size,
        height: size,
    }
}
