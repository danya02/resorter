use std::{fmt::Display, fs::OpenOptions, path::PathBuf};

use clap::{Parser, Subcommand};
use rand::{seq::SliceRandom, Rng};
use serde::{Deserialize, Serialize};
use skillratings::{
    glicko::{decay_deviation, glicko, GlickoConfig, GlickoRating},
    Outcomes,
};
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// Path to the CSV file to process.
    /// This file will be read, resorted, and then rewritten.
    #[arg(short = 'f', long, default_value = "items.csv")]
    file: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Add a new row to the file.
    Add {
        /// The name of the new row
        name: String,
    },

    /// Run resorting on existing file.
    Resort {
        /// Before starting the resorting, decay each rating by one step.
        /// This will ask you additional questions about items you have already sorted.
        #[arg(short, long)]
        decay: bool,
    },
}

fn main() {
    let args = Args::parse();
    match args.command {
        Commands::Add { name } => add_row_to_file(name, &args.file),
        Commands::Resort { decay } => run_resort(&args.file, decay),
    }
}

fn add_row_to_file(row: String, file: &PathBuf) {
    let opened = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .expect(&format!("Failed to open file {}", file.display()));
    let mut csv_writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(opened);
    csv_writer
        .write_record(&[&row, "1500.0", "100.0", "0"])
        .expect("Failed to write new row to file");
    println!("Added new record: {row:?}");
}

#[derive(Deserialize, Serialize)]
struct RatedItem {
    name: String,
    rating: f64,
    deviation: f64,
    rating_quartile: i64,
}

fn run_resort(file: &PathBuf, do_decay: bool) {
    println!("Loading ratings from disk...");
    let opened = OpenOptions::new()
        .read(true)
        .open(file)
        .expect(&format!("Failed to open file {}", file.display()));
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(opened);
    let mut items = vec![];
    for record in reader.records() {
        let record = record.expect("Failed to read record");
        let item: RatedItem = record.deserialize(None).expect("Failed to parse record");
        items.push(item);
    }

    if do_decay {
        println!("Processing rating decay...");
        for item in items.iter_mut() {
            let rating = GlickoRating {
                rating: item.rating,
                deviation: item.deviation,
            };
            let new_rating = decay_deviation(&rating, &GlickoConfig::default());
            item.rating = new_rating.rating;
            item.deviation = new_rating.deviation;
        }
    }

    if items.len() < 2 {
        println!("Cannot sort less than 2 items");
        return;
    }

    let rating_deviation_threshold = 65.0;

    // Shuffle the items so that they aren't presented in a predictable order.
    {
        let mut rng = rand::thread_rng();
        items.shuffle(&mut rng);
    }

    let mut unstabilized = items
        .iter()
        .filter(|v| v.deviation > rating_deviation_threshold)
        .count()
        > 0;

    while unstabilized {
        {
            let left;
            let right;
            if rand::random::<f64>() < 0.25 {
                // Most of the time, select the two top deviations.
                items.sort_unstable_by(|a, b| {
                    a.deviation
                        .partial_cmp(&b.deviation)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .reverse()
                });
                {
                    let mut iter = items.iter_mut();
                    left = iter.next().unwrap();
                    right = iter.next().unwrap();
                }
            } else {
                // A minority of the time, select two random items.
                let mut rng = rand::thread_rng();
                let (needed_items, other_items) = (items.partial_shuffle(&mut rng, 2));
                let (left_part, right_part) = needed_items.split_at_mut(1);
                left = &mut left_part[0];
                right = &mut right_part[0];
            }
            let left_player = GlickoRating {
                rating: left.rating,
                deviation: left.deviation,
            };
            let right_player = GlickoRating {
                rating: right.rating,
                deviation: right.deviation,
            };

            let left_name = format!("1. {}", left.name);
            let right_name = format!("2. {}", right.name);
            let mut q =
                inquire::Select::new("Which is better?", vec![&left_name, "equal", &right_name]);
            q.starting_cursor = 1;
            let answer = q.prompt().unwrap();
            let outcome = match answer {
                x if x == &left_name => Outcomes::WIN,
                x if x == &right_name => Outcomes::LOSS,
                _ => Outcomes::LOSS,
            };
            let (new_left_player, new_right_player) = glicko(
                &left_player,
                &right_player,
                &outcome,
                &GlickoConfig::default(),
            );
            left.rating = new_left_player.rating;
            left.deviation = new_left_player.deviation;
            right.rating = new_right_player.rating;
            right.deviation = new_right_player.deviation;
        }

        // Save the current ratings.
        save_ratings(file, &mut items);

        // Check if the ratings are now stabilized.
        unstabilized = false;
        for item in items.iter() {
            if item.deviation > rating_deviation_threshold {
                unstabilized = true;
                break;
            }
        }
    }
    println!("Ratings are stabilized!");
}

fn save_ratings(file: &PathBuf, items: &mut Vec<RatedItem>) {
    // Sort the items based on the rating.
    items.sort_unstable_by(|a, b| {
        a.rating
            .partial_cmp(&b.rating)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let quartiles = 10; // TODO: support different
    let items_per_quartile = items.len() / quartiles;
    let mut current_quartile = 0;
    let mut items_in_current_quartile = 0;
    for item in items.iter_mut() {
        item.rating_quartile = current_quartile;
        items_in_current_quartile += 1;
        if items_in_current_quartile > items_per_quartile {
            items_in_current_quartile = 0;
            current_quartile += 1;
        }
    }

    let new = file.with_extension("new");
    let opened = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&new)
        .expect(&format!("Failed to open file {}", new.display()));
    let mut csv_writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(opened);
    for item in items.iter() {
        csv_writer.serialize(item).expect("Failed to write record");
    }

    std::fs::rename(new, file).expect("Failed to replace old ratings list with new one");
}
