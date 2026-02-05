#![windows_subsystem = "windows"]

mod app;
mod hasher;
mod history;
mod robocopy;

use app::RoboAftApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let icon = create_icon();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([1000.0, 700.0])
            .with_title("Robo-AFT - Robocopy GUI Tool")
            .with_icon(icon),
        ..Default::default()
    };
    
    eframe::run_native(
        "Robo-AFT",
        options,
        Box::new(|cc| Ok(Box::new(RoboAftApp::new(cc)))),
    )
}

/// Create the application icon (cursive R) for the window/taskbar
fn create_icon() -> egui::IconData {
    let size = 64u32;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    
    // Colors (Material Design inspired)
    let bg_color = [0x1C, 0x1B, 0x1F, 0xFF];        // Dark surface
    let r_color = [0x4F, 0xC3, 0xF7, 0xFF];         // Primary cyan/teal
    let highlight = [0x80, 0xCB, 0xC4, 0xFF];       // Secondary teal
    
    let s = size as f32;
    
    // Fill background
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            pixels[idx..idx+4].copy_from_slice(&bg_color);
        }
    }
    
    // Draw a stylized cursive "R"
    let stroke_width = s * 0.11;
    let margin = s * 0.12;
    
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32;
            let fy = y as f32;
            
            let mut draw = false;
            let mut use_highlight = false;
            
            // Vertical stem (slightly curved for cursive feel)
            let stem_x = margin + s * 0.1;
            let curve_offset = ((fy - s / 2.0) / s).powi(2) * s * 0.05;
            if (fx - (stem_x + curve_offset)).abs() < stroke_width 
                && fy > margin && fy < s - margin {
                draw = true;
            }
            
            // Top loop (bowl) - elliptical arc
            let loop_center_x = s * 0.5;
            let loop_center_y = margin + s * 0.22;
            let loop_rx = s * 0.30;
            let loop_ry = s * 0.18;
            
            let dx = (fx - loop_center_x) / loop_rx;
            let dy = (fy - loop_center_y) / loop_ry;
            let dist = (dx * dx + dy * dy).sqrt();
            
            if (dist - 1.0).abs() < stroke_width / loop_rx * 1.3 {
                let angle = dy.atan2(dx);
                if angle > -std::f32::consts::PI * 0.85 && angle < std::f32::consts::PI * 0.45 {
                    draw = true;
                    if angle > 0.0 && angle < std::f32::consts::PI * 0.3 {
                        use_highlight = true;
                    }
                }
            }
            
            // Diagonal leg
            let leg_start_y = s * 0.5;
            let leg_start_x = stem_x + stroke_width * 0.5;
            
            if fy > leg_start_y && fy < s - margin * 0.7 {
                let progress = (fy - leg_start_y) / (s - margin - leg_start_y);
                let target_x = leg_start_x + progress * (s - margin * 1.3 - leg_start_x);
                let curve = (progress * std::f32::consts::PI).sin() * s * 0.025;
                let leg_x = target_x + curve;
                
                if (fx - leg_x).abs() < stroke_width * (0.85 + progress * 0.35) {
                    draw = true;
                    if progress > 0.6 {
                        use_highlight = true;
                    }
                }
            }
            
            // Flourish at bottom of leg
            let tail_center_x = s - margin * 1.1;
            let tail_center_y = s - margin * 0.9;
            let tail_r = s * 0.09;
            let tail_dist = ((fx - tail_center_x).powi(2) + (fy - tail_center_y).powi(2)).sqrt();
            if (tail_dist - tail_r).abs() < stroke_width * 0.55 {
                let angle = (fy - tail_center_y).atan2(fx - tail_center_x);
                if angle > std::f32::consts::PI * 0.15 && angle < std::f32::consts::PI * 1.15 {
                    draw = true;
                    use_highlight = true;
                }
            }
            
            // Entry flourish at top left
            let entry_x = margin * 0.6;
            let entry_y = margin * 1.4;
            let entry_dist = ((fx - entry_x).powi(2) + (fy - entry_y).powi(2)).sqrt();
            if entry_dist < s * 0.11 && entry_dist > s * 0.045 {
                let angle = (fy - entry_y).atan2(fx - entry_x);
                if angle > -std::f32::consts::PI * 0.25 && angle < std::f32::consts::PI * 0.65 {
                    draw = true;
                }
            }
            
            if draw {
                let idx = ((y * size + x) * 4) as usize;
                let color = if use_highlight { highlight } else { r_color };
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
