use clap::{Parser, Subcommand};
use server::serve;
use std::path::PathBuf;

mod backfill_lyricsfiles;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the LRCLIB server
    Serve {
        /// The port you want the server to bind to
        #[arg(short, long, value_name = "PORT", default_value_t = 3300)]
        port: u16,

        /// Path to the database file
        #[arg(short, long, value_name = "FILE", env = "LRCLIB_DATABASE_FILE")]
        database: PathBuf,
    },

    /// Populate Lyricsfile for historical lyrics rows
    BackfillLyricsfiles {
        /// Path to the database file
        #[arg(short, long, value_name = "FILE", env = "LRCLIB_DATABASE_FILE")]
        database: PathBuf,

        /// Maximum rows updated in one transaction
        #[arg(long, default_value_t = 500)]
        batch_size: usize,

        /// Delay between write batches; use zero for offline backfill
        #[arg(long, default_value_t = 50)]
        sleep_ms: u64,

        /// Stop after selecting this many rows
        #[arg(long)]
        limit: Option<usize>,

        /// Convert a sample and report results without writing
        #[arg(long)]
        dry_run: bool,

        /// Number of candidates converted during a dry run
        #[arg(long, default_value_t = 1000)]
        sample_size: usize,

        /// Report progress after this many selected rows
        #[arg(long, default_value_t = 10_000)]
        progress_every: usize,
    },
}

fn main() {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();

    if let Some(max_threads) = max_blocking_threads_from_env() {
        builder.max_blocking_threads(max_threads);
    }

    builder.build().unwrap().block_on(run());
}

fn max_blocking_threads_from_env() -> Option<usize> {
    std::env::var("LRCLIB_MAX_BLOCKING_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
}

async fn run() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Serve { port, database }) => {
            serve(port.to_owned(), database).await;
        }
        Some(Commands::BackfillLyricsfiles {
            database,
            batch_size,
            sleep_ms,
            limit,
            dry_run,
            sample_size,
            progress_every,
        }) => {
            let options = backfill_lyricsfiles::BackfillOptions {
                database: database.to_owned(),
                batch_size: *batch_size,
                sleep_ms: *sleep_ms,
                limit: *limit,
                dry_run: *dry_run,
                sample_size: *sample_size,
                progress_every: *progress_every,
            };
            if let Err(error) = backfill_lyricsfiles::run(options).await {
                eprintln!("Lyricsfile backfill failed: {error:#}");
                std::process::exit(1);
            }
        }
        None => {}
    }
}
