use std::time::Instant;

use xsynth_core::{
    soundfont::{SampleSoundfont, SoundfontInitOptions},
    AudioStreamParams, ChannelCount,
};

/// Approximate memory usage of the loaded soundfont.
/// This is a rough estimate based on the data structures used.
fn estimate_memory_mb(sf: &SampleSoundfont) -> f64 {
    // Use a trait object to get a debug print size approximation
    // We'll estimate based on what we know about the internal structure
    let size = std::mem::size_of_val(sf);
    size as f64 / (1024.0 * 1024.0)
}

fn main() {
    let sf2_path = r"D:\BM-DATA\SF2\Nexus Trap Piano V2（黑乐谱推荐）.sf2";

    println!("=== SF2 Soundfont Loading Speed Test ===");
    println!("File: {}", sf2_path);

    // Check file exists
    match std::fs::metadata(sf2_path) {
        Ok(meta) => println!("File size: {:.2} MB", meta.len() as f64 / (1024.0 * 1024.0)),
        Err(e) => {
            eprintln!("ERROR: Cannot access file: {}", e);
            return;
        }
    }

    let stream_params = AudioStreamParams::new(44100, ChannelCount::Stereo);
    let options = SoundfontInitOptions::default();

    println!("Sample rate: {}", stream_params.sample_rate);
    println!("Channels: Stereo");
    println!();

    // --- Phase 1: Full load test ---
    println!("--- Test 1: Full load (parse + build lookup table) ---");
    let start = Instant::now();
    let result = SampleSoundfont::new_sf2(sf2_path, stream_params, options);
    let full_load_time = start.elapsed();
    println!("Full load time: {:.3}s", full_load_time.as_secs_f64());

    let sf = match result {
        Ok(sf) => sf,
        Err(e) => {
            eprintln!("ERROR: Failed to load soundfont: {}", e);
            return;
        }
    };

    // --- Phase 2: Measure memory (via process info on Windows) ---
    let mem_mb = get_process_memory_mb();
    println!("Process memory after load: {:.2} MB", mem_mb);
    println!("Struct size estimate: {:.6} MB", estimate_memory_mb(&sf));
    println!();

    // --- Phase 3: Multiple runs for average ---
    println!("--- Test 2: Multiple load runs (5 iterations) ---");
    let mut times: Vec<std::time::Duration> = Vec::new();
    for i in 0..5 {
        let start = Instant::now();
        let _ = SampleSoundfont::new_sf2(sf2_path, stream_params, options);
        let elapsed = start.elapsed();
        times.push(elapsed);
        println!("  Run {}: {:.3}s", i + 1, elapsed.as_secs_f64());
    }
    let avg_time = times.iter().sum::<std::time::Duration>() / times.len() as u32;
    let min_time = times.iter().min().unwrap();
    let max_time = times.iter().max().unwrap();
    println!(
        "  Average: {:.3}s | Min: {:.3}s | Max: {:.3}s",
        avg_time.as_secs_f64(),
        min_time.as_secs_f64(),
        max_time.as_secs_f64()
    );
    println!();

    // --- Phase 4: Parse-only test ---
    println!("--- Test 3: Parse only (xsynth-soundfonts layer) ---");
    let start = Instant::now();
    let parse_result = xsynth_soundfonts::sf2::load_soundfont(sf2_path, stream_params.sample_rate);
    let parse_time = start.elapsed();
    println!("Parse time: {:.3}s", parse_time.as_secs_f64());

    match parse_result {
        Ok(presets) => {
            let total_samples: usize = presets.iter().map(|p| p.regions.len()).sum();
            println!("Presets: {}", presets.len());
            println!("Total regions across all presets: {}", total_samples);

            // Estimate sample data memory
            let sample_mem: usize = presets
                .iter()
                .flat_map(|p| p.regions.iter())
                .map(|r| r.sample.iter().map(|s| s.len() * 4).sum::<usize>())
                .sum();
            println!(
                "Estimated sample data memory: {:.2} MB",
                sample_mem as f64 / (1024.0 * 1024.0)
            );
        }
        Err(e) => {
            eprintln!("ERROR: Parse failed: {}", e);
        }
    }
    println!();

    // File size for memory limit calculation (1.5x original file size)
    let file_size_mb = std::fs::metadata(sf2_path)
        .map(|m| m.len() as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0);
    let mem_limit_mb = file_size_mb * 1.5;
    let time_limit_s = 0.5;

    // --- Summary ---
    println!("=== Summary ===");
    let time_passes = full_load_time.as_secs_f64() <= time_limit_s;
    let mem_passes = mem_mb <= mem_limit_mb;
    println!(
        "Load time: {:.3}s {} (limit: {:.1}s)",
        full_load_time.as_secs_f64(),
        if time_passes { "PASS" } else { "FAIL" },
        time_limit_s
    );
    println!(
        "Memory usage: {:.2} MB {} (limit: {:.0} MB = 1.5x file size)",
        mem_mb,
        if mem_passes { "PASS" } else { "FAIL" },
        mem_limit_mb
    );
    println!(
        "Average load time (5 runs): {:.3}s {}",
        avg_time.as_secs_f64(),
        if avg_time.as_secs_f64() <= time_limit_s {
            "PASS"
        } else {
            "FAIL"
        }
    );

    if time_passes && mem_passes {
        println!("\n✓ All tests PASSED");
    } else {
        println!("\n✗ Some tests FAILED - optimization needed");
    }
}

#[cfg(target_os = "windows")]
fn get_process_memory_mb() -> f64 {
    use std::mem;

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            pmc: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    unsafe {
        let mut pmc = ProcessMemoryCounters {
            cb: mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };

        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc,
            mem::size_of::<ProcessMemoryCounters>() as u32,
        );

        pmc.working_set_size as f64 / (1024.0 * 1024.0)
    }
}

#[cfg(not(target_os = "windows"))]
fn get_process_memory_mb() -> f64 {
    // Fallback for non-Windows: read from /proc/self/status
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let kb: usize = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                return kb as f64 / 1024.0;
            }
        }
    }
    0.0
}
