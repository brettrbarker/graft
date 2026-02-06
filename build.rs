use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    // Only run on Windows
    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let icon_path = Path::new(&out_dir).join("icon.ico");
    
    // Generate the icon file
    generate_icon(&icon_path);
    
    // Use winresource to embed the icon
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon_path.to_str().unwrap());
    res.set("ProductName", "GRAFT");
    res.set("FileDescription", "Graphical Robocopy Assured File Transfer Tool");
    res.set("LegalCopyright", "Copyright © 2026 Brett Barker");
    
    if let Err(e) = res.compile() {
        eprintln!("Warning: Failed to compile Windows resources: {}", e);
    }
}

/// Generate a 256x256 + 48x48 + 32x32 + 16x16 ICO file with a stylized "G" design
fn generate_icon(path: &Path) {
    let mut file = File::create(path).expect("Failed to create icon file");
    
    // We'll create a multi-size ICO with 256, 48, 32, and 16 pixel versions
    let sizes = [256u32, 48, 32, 16];
    
    // ICO Header
    let num_images = sizes.len() as u16;
    file.write_all(&[0, 0]).unwrap(); // Reserved
    file.write_all(&[1, 0]).unwrap(); // Type: 1 = ICO
    file.write_all(&num_images.to_le_bytes()).unwrap();
    
    // Calculate offsets for each image
    let header_size = 6 + (16 * num_images as u32);
    let mut current_offset = header_size;
    let mut image_data: Vec<Vec<u8>> = Vec::new();
    
    for &size in &sizes {
        let png_data = generate_png_g(size);
        image_data.push(png_data);
    }
    
    // Write ICONDIRENTRY for each image
    for (i, &size) in sizes.iter().enumerate() {
        let width = if size >= 256 { 0u8 } else { size as u8 };
        let height = if size >= 256 { 0u8 } else { size as u8 };
        let data_size = image_data[i].len() as u32;
        
        file.write_all(&[width]).unwrap();      // Width
        file.write_all(&[height]).unwrap();     // Height
        file.write_all(&[0]).unwrap();          // Color palette
        file.write_all(&[0]).unwrap();          // Reserved
        file.write_all(&[1, 0]).unwrap();       // Color planes
        file.write_all(&[32, 0]).unwrap();      // Bits per pixel
        file.write_all(&data_size.to_le_bytes()).unwrap();
        file.write_all(&current_offset.to_le_bytes()).unwrap();
        
        current_offset += data_size;
    }
    
    // Write image data
    for data in &image_data {
        file.write_all(data).unwrap();
    }
}

/// Generate a PNG image with a stylized "G" design
fn generate_png_g(size: u32) -> Vec<u8> {
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
    // The G consists of:
    // 1. A large open arc (C-shape) — the main body
    // 2. A horizontal crossbar extending inward from the right at mid-height
    
    let stroke_width = s * 0.11;
    let radius = s * 0.34;
    
    // Gap angle: the opening of the C faces right, from about -45° to +45°
    let gap_start = -std::f32::consts::PI * 0.28; // ~-50 degrees
    let gap_end = std::f32::consts::PI * 0.28;     // ~+50 degrees
    
    // Crossbar parameters
    let bar_y_center = center_y;
    let bar_left = center_x - s * 0.02;  // extends slightly past center
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
                // Draw the arc everywhere except the gap (opening on the right)
                if angle < gap_start || angle > gap_end {
                    draw = true;
                    // Highlight the top portion of the arc
                    if angle < -std::f32::consts::PI * 0.4 && angle > -std::f32::consts::PI * 0.9 {
                        use_highlight = true;
                    }
                }
            }
            
            // Horizontal crossbar (the tongue of the G)
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
    
    // Apply anti-aliasing by simple blur on edges
    let mut result = pixels.clone();
    for y in 1..(size-1) {
        for x in 1..(size-1) {
            let idx = ((y * size + x) * 4) as usize;
            let is_edge = pixels[idx] != pixels[((y * size + x + 1) * 4) as usize]
                || pixels[idx] != pixels[((y * size + x - 1) * 4) as usize]
                || pixels[idx] != pixels[(((y+1) * size + x) * 4) as usize]
                || pixels[idx] != pixels[(((y-1) * size + x) * 4) as usize];
            
            if is_edge {
                for c in 0..3 {
                    let mut sum = 0u32;
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let ny = (y as i32 + dy) as u32;
                            let nx = (x as i32 + dx) as u32;
                            let nidx = ((ny * size + nx) * 4) as usize;
                            sum += pixels[nidx + c] as u32;
                        }
                    }
                    result[idx + c] = (sum / 9) as u8;
                }
            }
        }
    }
    
    // Encode as PNG
    encode_png(&result, size, size)
}

/// Simple PNG encoder (uncompressed for simplicity, but valid PNG)
fn encode_png(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut png = Vec::new();
    
    // PNG signature
    png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    
    // IHDR chunk
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8);  // bit depth
    ihdr.push(6);  // color type: RGBA
    ihdr.push(0);  // compression
    ihdr.push(0);  // filter
    ihdr.push(0);  // interlace
    write_chunk(&mut png, b"IHDR", &ihdr);
    
    // IDAT chunk (image data)
    // Prepare raw image data with filter bytes
    let mut raw_data = Vec::new();
    for y in 0..height {
        raw_data.push(0); // Filter type: None
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            raw_data.extend_from_slice(&pixels[idx..idx+4]);
        }
    }
    
    // Compress with zlib (deflate)
    let compressed = compress_zlib(&raw_data);
    write_chunk(&mut png, b"IDAT", &compressed);
    
    // IEND chunk
    write_chunk(&mut png, b"IEND", &[]);
    
    png
}

fn write_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    let len = data.len() as u32;
    png.extend_from_slice(&len.to_be_bytes());
    png.extend_from_slice(chunk_type);
    png.extend_from_slice(data);
    
    // CRC32
    let mut crc_data = Vec::new();
    crc_data.extend_from_slice(chunk_type);
    crc_data.extend_from_slice(data);
    let crc = crc32(&crc_data);
    png.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = CRC32_TABLE[idx] ^ (crc >> 8);
    }
    !crc
}

const CRC32_TABLE: [u32; 256] = [
    0x00000000, 0x77073096, 0xee0e612c, 0x990951ba, 0x076dc419, 0x706af48f, 0xe963a535, 0x9e6495a3,
    0x0edb8832, 0x79dcb8a4, 0xe0d5e91e, 0x97d2d988, 0x09b64c2b, 0x7eb17cbd, 0xe7b82d07, 0x90bf1d91,
    0x1db71064, 0x6ab020f2, 0xf3b97148, 0x84be41de, 0x1adad47d, 0x6ddde4eb, 0xf4d4b551, 0x83d385c7,
    0x136c9856, 0x646ba8c0, 0xfd62f97a, 0x8a65c9ec, 0x14015c4f, 0x63066cd9, 0xfa0f3d63, 0x8d080df5,
    0x3b6e20c8, 0x4c69105e, 0xd56041e4, 0xa2677172, 0x3c03e4d1, 0x4b04d447, 0xd20d85fd, 0xa50ab56b,
    0x35b5a8fa, 0x42b2986c, 0xdbbbc9d6, 0xacbcf940, 0x32d86ce3, 0x45df5c75, 0xdcd60dcf, 0xabd13d59,
    0x26d930ac, 0x51de003a, 0xc8d75180, 0xbfd06116, 0x21b4f4b5, 0x56b3c423, 0xcfba9599, 0xb8bda50f,
    0x2802b89e, 0x5f058808, 0xc60cd9b2, 0xb10be924, 0x2f6f7c87, 0x58684c11, 0xc1611dab, 0xb6662d3d,
    0x76dc4190, 0x01db7106, 0x98d220bc, 0xefd5102a, 0x71b18589, 0x06b6b51f, 0x9fbfe4a5, 0xe8b8d433,
    0x7807c9a2, 0x0f00f934, 0x9609a88e, 0xe10e9818, 0x7f6a0dbb, 0x086d3d2d, 0x91646c97, 0xe6635c01,
    0x6b6b51f4, 0x1c6c6162, 0x856530d8, 0xf262004e, 0x6c0695ed, 0x1b01a57b, 0x8208f4c1, 0xf50fc457,
    0x65b0d9c6, 0x12b7e950, 0x8bbeb8ea, 0xfcb9887c, 0x62dd1ddf, 0x15da2d49, 0x8cd37cf3, 0xfbd44c65,
    0x4db26158, 0x3ab551ce, 0xa3bc0074, 0xd4bb30e2, 0x4adfa541, 0x3dd895d7, 0xa4d1c46d, 0xd3d6f4fb,
    0x4369e96a, 0x346ed9fc, 0xad678846, 0xda60b8d0, 0x44042d73, 0x33031de5, 0xaa0a4c5f, 0xdd0d7a89,
    0x5005713c, 0x270241aa, 0xbe0b1010, 0xc90c2086, 0x5768b525, 0x206f85b3, 0xb966d409, 0xce61e49f,
    0x5edef90e, 0x29d9c998, 0xb0d09822, 0xc7d7a8b4, 0x59b33d17, 0x2eb40d81, 0xb7bd5c3b, 0xc0ba6cad,
    0xedb88320, 0x9abfb3b6, 0x03b6e20c, 0x74b1d29a, 0xead54739, 0x9dd277af, 0x04db2615, 0x73dc1683,
    0xe3630b12, 0x94643b84, 0x0d6d6a3e, 0x7a6a5aa8, 0xe40ecf0b, 0x9309ff9d, 0x0a00ae27, 0x7d079eb1,
    0xf00f9344, 0x8708a3d2, 0x1e01f268, 0x6906c2fe, 0xf762575d, 0x806567cb, 0x196c3671, 0x6e6b06e7,
    0xfed41b76, 0x89d32be0, 0x10da7a5a, 0x67dd4acc, 0xf9b9df6f, 0x8ebeeff9, 0x17b7be43, 0x60b08ed5,
    0xd6d6a3e8, 0xa1d1937e, 0x38d8c2c4, 0x4fdff252, 0xd1bb67f1, 0xa6bc5767, 0x3fb506dd, 0x48b2364b,
    0xd80d2bda, 0xaf0a1b4c, 0x36034af6, 0x41047a60, 0xdf60efc3, 0xa867df55, 0x316e8eef, 0x4669be79,
    0xcb61b38c, 0xbc66831a, 0x256fd2a0, 0x5268e236, 0xcc0c7795, 0xbb0b4703, 0x220216b9, 0x5505262f,
    0xc5ba3bbe, 0xb2bd0b28, 0x2bb45a92, 0x5cb36a04, 0xc2d7ffa7, 0xb5d0cf31, 0x2cd99e8b, 0x5bdeae1d,
    0x9b64c2b0, 0xec63f226, 0x756aa39c, 0x026d930a, 0x9c0906a9, 0xeb0e363f, 0x72076785, 0x05005713,
    0x95bf4a82, 0xe2b87a14, 0x7bb12bae, 0x0cb61b38, 0x92d28e9b, 0xe5d5be0d, 0x7cdcefb7, 0x0bdbdf21,
    0x86d3d2d4, 0xf1d4e242, 0x68ddb3f8, 0x1fda836e, 0x81be16cd, 0xf6b9265b, 0x6fb077e1, 0x18b74777,
    0x88085ae6, 0xff0f6a70, 0x66063bca, 0x11010b5c, 0x8f659eff, 0xf862ae69, 0x616bffd3, 0x166ccf45,
    0xa00ae278, 0xd70dd2ee, 0x4e048354, 0x3903b3c2, 0xa7672661, 0xd06016f7, 0x4969474d, 0x3e6e77db,
    0xaed16a4a, 0xd9d65adc, 0x40df0b66, 0x37d83bf0, 0xa9bcae53, 0xdede86c5, 0x47d7977f, 0x30d0a6e9,
    0xbdd3d20c, 0xcad4e29a, 0x53ddb020, 0x24da80b6, 0xbabe3515, 0xcdbb0583, 0x54b25439, 0x23b564af,
    0xb3c0793e, 0xc4c748a8, 0x5dce5912, 0x2ac96984, 0x94bdd727, 0xe3bae7b1, 0x7ab38c0b, 0x0db4fc9d,
];

/// Simple zlib compression (store only, no actual compression for simplicity)
fn compress_zlib(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    
    // Zlib header (no compression)
    result.push(0x78); // CMF
    result.push(0x01); // FLG
    
    // Deflate blocks
    let chunk_size = 65535;
    let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
    
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == chunks.len() - 1;
        result.push(if is_last { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00
        
        let len = chunk.len() as u16;
        let nlen = !len;
        result.extend_from_slice(&len.to_le_bytes());
        result.extend_from_slice(&nlen.to_le_bytes());
        result.extend_from_slice(chunk);
    }
    
    // Adler-32 checksum
    let adler = adler32(data);
    result.extend_from_slice(&adler.to_be_bytes());
    
    result
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    
    (b << 16) | a
}
