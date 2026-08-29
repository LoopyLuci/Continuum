use serde::Deserialize;
use std::io::{BufReader, Read};
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RecordedFrameHeader {
    timestamp: String,
    data_len: u32,
}

pub fn run_replay(file_path: &PathBuf) -> anyhow::Result<()> {
    let file = std::fs::File::open(file_path)?;
    let mut reader = BufReader::new(file);

    let mut frame_count = 0u64;
    let mut total_bytes = 0u64;
    let mut last_timestamp: Option<String> = None;
    let mut input_count = 0u64;

    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }

        let header_len = u32::from_be_bytes(len_buf) as usize;

        if header_len == 0xFFFFFFFF {
            let mut input_len_buf = [0u8; 4];
            reader.read_exact(&mut input_len_buf)?;
            let input_len = u32::from_be_bytes(input_len_buf) as usize;
            let mut input_data = vec![0u8; input_len];
            reader.read_exact(&mut input_data)?;
            input_count += 1;
            continue;
        }

        if header_len > 1024 * 1024 {
            anyhow::bail!("Invalid header length: {}", header_len);
        }

        let mut header_data = vec![0u8; header_len];
        reader.read_exact(&mut header_data)?;

        reader.read_exact(&mut len_buf)?;
        let data_len = u32::from_be_bytes(len_buf) as usize;

        if data_len > 64 * 1024 * 1024 {
            anyhow::bail!("Invalid frame data length: {}", data_len);
        }

        let mut frame_data = vec![0u8; data_len];
        reader.read_exact(&mut frame_data)?;

        if let Ok(header) = serde_json::from_slice::<RecordedFrameHeader>(&header_data) {
            let ts = header.timestamp;
            if let Some(ref prev) = last_timestamp {
                if &ts < prev {
                    eprintln!(
                        "WARNING: Timestamp not monotonic at frame {}",
                        frame_count + 1
                    );
                }
            }
            last_timestamp = Some(ts);
        }

        frame_count += 1;
        total_bytes += data_len as u64;

        match image::load_from_memory(&frame_data) {
            Ok(img) => {
                if frame_count <= 3 || frame_count.is_multiple_of(50) {
                    eprintln!(
                        "  Frame {}: {}x{}, {:.1} KB",
                        frame_count,
                        img.width(),
                        img.height(),
                        data_len as f64 / 1024.0
                    );
                }
            }
            Err(e) => {
                eprintln!("  Frame {}: invalid JPEG: {}", frame_count, e);
            }
        }
    }

    eprintln!("\nReplay complete:");
    eprintln!("  Frames: {}", frame_count);
    eprintln!("  Inputs: {}", input_count);
    eprintln!(
        "  Total data: {:.2} MB",
        total_bytes as f64 / (1024.0 * 1024.0)
    );
    eprintln!("  File: {}", file_path.display());

    Ok(())
}
