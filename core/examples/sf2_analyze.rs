use std::time::Instant;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::collections::HashMap;

fn main() {
    let path = "D:\\BM-DATA\\SF2\\Nexus Trap Piano V2（黑乐谱推荐）.sf2";

    // ---- Phase 1: File open + SF2 header parse ----
    let t0 = Instant::now();
    let mut file = File::open(path).unwrap();
    let sf2 = soundfont::SoundFont2::load(&mut file).unwrap();
    let t_header = t0.elapsed();
    println!("SF2 header parse: {:.2?}ms", t_header.as_secs_f64() * 1000.0);
    println!("  Sample headers: {}", sf2.sample_headers.len());
    println!("  Presets: {}", sf2.presets.len());

    // Analyze sample rates
    let mut rate_counts: HashMap<u32, usize> = HashMap::new();
    for h in &sf2.sample_headers {
        *rate_counts.entry(h.sample_rate).or_insert(0) += 1;
    }
    println!("\nSample rate distribution:");
    for (rate, count) in &rate_counts {
        println!("  {} Hz: {} samples", rate, count);
    }

    let needs_resample = sf2.sample_headers.iter().any(|h| h.sample_rate != 44100);
    println!("\nNeeds resampling for 44100Hz target: {}", needs_resample);

    // ---- Phase 2: Raw sample read ----
    let smpl_chunk = sf2.sample_data.smpl.as_ref().unwrap();
    let t1 = Instant::now();
    let mut raw_buf = vec![0u8; smpl_chunk.len as usize];
    file.seek(SeekFrom::Start(smpl_chunk.offset)).unwrap();
    file.read_exact(&mut raw_buf).unwrap();
    let t_read = t1.elapsed();
    println!("\nRaw sample read ({:.1} MB): {:.2?}ms", smpl_chunk.len as f64 / 1024.0 / 1024.0, t_read.as_secs_f64() * 1000.0);

    // ---- Phase 3: i16 -> f32 conversion ----
    let t2 = Instant::now();
    let all_samples: Vec<f32> = raw_buf
        .chunks_exact(2)
        .map(|c| {
            let s = i16::from_le_bytes([c[0], c[1]]);
            s as f32 / i16::MAX as f32
        })
        .collect();
    let t_conv = t2.elapsed();
    println!("i16->f32 ({} samples, {:.1} MB): {:.2?}ms", 
        all_samples.len(), all_samples.len() as f64 * 4.0 / 1024.0 / 1024.0, 
        t_conv.as_secs_f64() * 1000.0);

    // ---- Phase 4: Arc slicing (fast path) ----
    let t3 = Instant::now();
    let slices: Vec<std::sync::Arc<[f32]>> = sf2.sample_headers.iter()
        .map(|h| all_samples[h.start as usize..h.end as usize].into())
        .collect();
    let t_slice = t3.elapsed();
    println!("Arc slicing ({} slices): {:.2?}ms", slices.len(), t_slice.as_secs_f64() * 1000.0);

    // ---- Phase 5: Simulate resampling (if needed) ----
    if needs_resample {
        use xsynth_soundfonts::resample::resample_vec;
        let t4 = Instant::now();
        let resampled: Vec<std::sync::Arc<[f32]>> = sf2.sample_headers.iter()
            .filter(|h| h.sample_rate != 44100)
            .map(|h| {
                let start = h.start as usize;
                let end = h.end as usize;
                let sample_data: Vec<f32> = all_samples[start..end].to_vec();
                resample_vec(sample_data, h.sample_rate as f32, 44100.0)
            })
            .collect();
        let t_resamp = t4.elapsed();
        println!("Resampling ({} samples): {:.2?}ms", resampled.len(), t_resamp.as_secs_f64() * 1000.0);
    }

    // ---- Phase 6: Full load via xsynth ----
    println!("\n--- Full xsynth load ---");
    let t5 = Instant::now();
    let presets = xsynth_soundfonts::sf2::load_soundfont(path, 44100).unwrap();
    let t_full = t5.elapsed();
    println!("Full load: {:.2?}ms", t_full.as_secs_f64() * 1000.0);
    println!("Presets: {}, Regions: {}", presets.len(), presets.iter().map(|p| p.regions.len()).sum::<usize>());
}
