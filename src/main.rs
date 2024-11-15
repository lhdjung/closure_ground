use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use csv::WriterBuilder;
use rayon::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Clone)]
struct Combination {
    values: Vec<i32>,
    running_sum: f64,
    running_m2: f64,
}

/// Calculates the number of initial combinations that will be processed in parallel
fn count_initial_combinations(min_scale: i32, max_scale: i32) -> i32 {
    // Each combination starts with two numbers i,j where min_scale ≤ i ≤ j ≤ max_scale
    // This is equivalent to choosing 2 numbers with replacement where order doesn't matter
    // The formula is: (n+1) * n / 2 where n is the range size
    let range_size = max_scale - min_scale + 1;
    (range_size * (range_size + 1)) / 2
}

/// DFS implementation that collects all valid combinations from a starting point
fn dfs_branch(
    start_combination: Vec<i32>,
    running_sum_init: f64,
    running_m2_init: f64,
    n: usize,
    target_sum_upper: f64,
    target_sum_lower: f64,
    target_sd_upper: f64,
    target_sd_lower: f64,
    min_scale_sum: &[i32],
    max_scale_sum: &[i32],
    n_1: usize,
    max_scale_1: i32,
) -> Vec<Vec<i32>> {
    let mut stack = VecDeque::new();
    let mut results = Vec::new();
    
    // Initialize stack with starting combination
    stack.push_back(Combination {
        values: start_combination,
        running_sum: running_sum_init,
        running_m2: running_m2_init,
    });
    
    while let Some(current) = stack.pop_back() {
        // Check if we've reached desired length
        if current.values.len() >= n {
            let current_std = (current.running_m2 / n_1 as f64).sqrt();
            if current_std >= target_sd_lower {
                results.push(current.values);
            }
            continue;
        }

        // Calculate remaining positions to fill
        let n_left = n_1 - current.values.len();
        let next_n = current.values.len() + 1;
        let last_value = *current.values.last().unwrap();

        // Try each possible next value
        for next_value in last_value..max_scale_1 {
            // Early pruning based on mean bounds
            let next_sum = current.running_sum + next_value as f64;
            let minmean = next_sum + min_scale_sum[n_left] as f64;
            if minmean > target_sum_upper {
                break; // No need to try larger values
            }
            
            let maxmean = next_sum + max_scale_sum[n_left] as f64;
            if maxmean < target_sum_lower {
                continue;
            }

            // Calculate standard deviation metrics
            let next_mean = next_sum / next_n as f64;
            let delta = next_value as f64 - current.running_sum / current.values.len() as f64;
            let delta2 = next_value as f64 - next_mean;
            let next_m2 = current.running_m2 + delta * delta2;
            
            // Early pruning based on standard deviation
            let min_sd = (next_m2 / n_1 as f64).sqrt();
            if min_sd > target_sd_upper {
                continue;
            }

            // Add valid combination to stack
            let mut new_values = current.values.clone();
            new_values.push(next_value);
            stack.push_back(Combination {
                values: new_values,
                running_sum: next_sum,
                running_m2: next_m2,
            });
        }
    }

    results
}

fn parallel_dfs(
    min_scale: i32,
    max_scale: i32,
    n: usize,
    target_sum: f64,
    target_sd: f64,
    rounding_error_sums: f64,
    rounding_error_sds: f64,
    output_file: &str,
) -> io::Result<()> {
    let start_time = Instant::now();
    
    // Calculate bounds for target metrics
    let target_sum_upper = target_sum + rounding_error_sums;
    let target_sum_lower = target_sum - rounding_error_sums;
    let target_sd_upper = target_sd + rounding_error_sds;
    let target_sd_lower = target_sd - rounding_error_sds;
    
    // Precompute scale sums for optimization
    let min_scale_sum: Vec<i32> = (0..n)
        .map(|x| min_scale * x as i32)
        .collect();
    let max_scale_sum: Vec<i32> = (0..n)
        .map(|x| max_scale * x as i32)
        .collect();
    
    let n_1 = n - 1;
    let max_scale_1 = max_scale + 1;

    // Generate initial combinations for parallel processing
    let mut initial_combinations = Vec::new();
    for i in min_scale..=max_scale {
        for j in i..=max_scale {
            let initial_combination = vec![i, j];
            let running_sum = (i + j) as f64;
            let current_mean = running_sum / 2.0;
            let current_m2 = (i as f64 - current_mean).powi(2) + 
                            (j as f64 - current_mean).powi(2);
            initial_combinations.push((initial_combination, running_sum, current_m2));
        }
    }

    // Initialize CSV file with headers
    let file = File::create(output_file)?;
    let mut writer = WriterBuilder::new()
        .has_headers(true)
        .from_writer(file);

    // Write header row
    let header: Vec<String> = (1..=n)
        .map(|i| format!("n{}", i))
        .collect();
    writer.write_record(&header)?;
    writer.flush()?;

    // Initialize progress bar
    let bar = ProgressBar::new(initial_combinations.len() as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-")
    );

    // Setup shared writer for parallel processing
    let writer = Arc::new(Mutex::new(
        WriterBuilder::new()
            .has_headers(false)
            .from_writer(
                OpenOptions::new()
                    .write(true)
                    .append(true)
                    .open(output_file)?
            )
    ));

    // Process combinations in parallel
    initial_combinations
        .par_iter()
        .for_each(|(combo, running_sum, running_m2)| {
            let results = dfs_branch(
                combo.clone(),
                *running_sum,
                *running_m2,
                n,
                target_sum_upper,
                target_sum_lower,
                target_sd_upper,
                target_sd_lower,
                &min_scale_sum,
                &max_scale_sum,
                n_1,
                max_scale_1,
            );

            // Write all results from this branch at once
            if !results.is_empty() {
                let mut writer = writer.lock().unwrap();
                for result in results {
                    writer
                        .write_record(
                            &result
                                .iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<String>>()
                        )
                        .unwrap();
                }
                writer.flush().unwrap();
            }
            bar.inc(1);
    });

    bar.finish_with_message("Done!");

    // Print execution time
    let duration = start_time.elapsed();
    println!("Execution time: {:.2} seconds", duration.as_secs_f64());

    // Count and print total valid combinations
    let file = File::open(output_file)?;
    let reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(file);
    let count = reader.into_records().count();
    println!("Number of valid combinations: {}", count);

    Ok(())
}

fn main() -> io::Result<()> {
    let min_scale = 1;
    let max_scale = 7;
    let n = 30;
    let target_mean = 5.0;
    let target_sum = target_mean * n as f64;
    let target_sd = 2.78;
    let rounding_error_means = 0.01;
    let rounding_error_sums = rounding_error_means * n as f64;
    let rounding_error_sds = 0.01;
    let output_file = "parallel_results.csv";

    // Calculate and print the number of initial parallel tasks
    let initial_count = count_initial_combinations(min_scale, max_scale);
    println!("Number of initial combinations to process: {}", initial_count);

    parallel_dfs(
        min_scale,
        max_scale,
        n,
        target_sum,
        target_sd,
        rounding_error_sums,
        rounding_error_sds,
        output_file,
    )
}