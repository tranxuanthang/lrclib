use std::path::PathBuf;
use clap::{Parser, Subcommand};
use server::serve;

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
    #[arg(
      short,
      long,
      value_name = "FILE",
      env = "LRCLIB_DATABASE_FILE"
    )]
    database: PathBuf,

    /// The number of queue processing workers
    #[arg(
      short,
      long,
      value_name = "WORKERS_COUNT",
      env = "LRCLIB_WORKERS_COUNT",
      default_value_t = 0
    )]
    workers_count: u8,
  },
}


#[tokio::main]
async fn main() {
  let cli = Cli::parse();

  match &cli.command {
    Some(Commands::Serve { port, database, workers_count }) => {
      serve(port.to_owned(), database, workers_count.to_owned()).await;
    },
    None => {}
  }
}
