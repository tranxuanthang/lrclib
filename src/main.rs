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
    /// The port you want the server to bind to.
    #[arg(short, long, value_name = "PORT")]
    port: u16,

    /// Path to the database file
    #[arg(short, long, value_name = "FILE")]
    database: PathBuf,
  },
}


#[tokio::main]
async fn main() {
  let cli = Cli::parse();

  match &cli.command {
    Some(Commands::Serve { port, database }) => {
      serve(port.to_owned(), database).await;
    },
    None => {}
  }
}
