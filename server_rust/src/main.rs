#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use docopt::Docopt;
use std::thread;
use std::io::Error;
use std::path::{PathBuf};

// For termination signals
use std::sync::atomic::{AtomicBool, Ordering};
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::flag;
use std::sync::Arc;

use crossbeam_channel::unbounded;   // Work queue

// For logging
use tracing::{info, error, warn};
use tracing_subscriber::FmtSubscriber;

mod video_pipeline;
mod api_server;


const USAGE: &'static str = r#"
Clapshot server - backend of a video annotation tool

Monitors <path>/incoming for new videos, processes them, and stores them in <path>/videos.
Then serves the annotations and comments via an asyncronous HTTP + Socket.IO API.
Use a proxy server to serve files in /videos and to secure the API with HTTPS/WSS.

Usage:
  clapshot-server [options] (--url-base=URL) (--data-dir=PATH)
  clapshot-server [options] [--mute TOPIC]... (--url-base=URL) (--data-dir=PATH)
  clapshot-server (-h | --help)

Required:
 --url-base=URL       Base URL of the API server, e.g. https://example.com/clapshot/.
                      This depends on your proxy server configuration.
 --data-dir=PATH      Directory for database, /incoming, /videos and /rejected

Options:
 -p PORT --port=PORT    Port to listen on [default: 8095]
 -H HOST --host=HOST    Host to listen on [default: 0.0.0.0]
 --host-videos          Host the /videos directory
                        (For debugging. Use Nginx or Apache with auth in production.)
 -P SEC --poll SEC      Polling interval for incoming folder [default: 3.0]
 -m TOPIC --mute TOPIC    Mute logging for a topic (can be repeated). Sets level to WARNING.
                        See logs logs for available topics.
 -l FILE --log FILE     Log to file instead of stdout
 -w N --workers N       Max number of workers for video processing [default: 0]
                        (0 = number of CPU cores)
 -d --debug             Enable debug logging
 -h --help              Show this screen
"#;


fn main() -> Result<(), Error>
{
    // Setup logging
    //if std::env::var_os("RUST_LOG").is_none() {
    //    std::env::set_var("RUST_LOG", "debug") };

    //let subscriber = FmtSubscriber::builder()
    //    .with_max_level(tracing::Level::TRACE).finish();

    let log_sbsc = tracing_subscriber::fmt()
        //.compact() // for production
        .pretty() // for development
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .finish();
    tracing::subscriber::set_global_default(log_sbsc).expect("tracing::subscriber::set_global_default failed");


    // Setup SIGINT / SIGTERM handling
    let terminate_flag = Arc::new(AtomicBool::new(false));
    for sig in TERM_SIGNALS {
        // Exit immediate on a second signal (e.g. double CTRL-C)
        flag::register_conditional_shutdown(*sig, 1, Arc::clone(&terminate_flag))?;
        // Set flag on first signal
        flag::register(*sig, Arc::clone(&terminate_flag))?;
    }


    println!("Hello, world!?!");

    let argv = || vec!["server_rust", "--url-base", "http://localhost", "--data-dir", "."];

    tracing::info!("Parsing command line arguments");

    let args = Docopt::new(USAGE)
        .and_then(|d| d.argv(argv().into_iter()).parse())
        .unwrap_or_else(|e| e.exit());

    let port_str = args.get_str("--port");
    let port = port_str.parse::<u16>().unwrap();
    println!("port: {}", port);

    let data_dir = PathBuf::from(args.get_str("--data-dir"));

    // spawn a thread to run the video processing pipeline
    let (vpp_thread, vpp_out) = {
        let (out_q, in_q) = unbounded::<video_pipeline::Args>();
        let th = thread::spawn(move || {
                video_pipeline::run_forever(
                    data_dir, 3.0, 15.0, in_q);
            }); 
        (th, out_q)
    };

    let tf = Arc::clone(&terminate_flag);
    let api_thread = thread::spawn(move || {
            if let Err(e) = api_server::run_forever(tf.clone(), 3030) {
                error!("API server failed: {}", e);
                tf.store(true, Ordering::Relaxed);
            }});

    // Loop forever, abort on SIGINT/SIGTERM or if child threads die
    while !terminate_flag.load(Ordering::Relaxed)
    {
        tracing::info!("Hello, world! (from main thread)");
        thread::sleep(std::time::Duration::from_secs(1));

        if vpp_thread.is_finished() {
            tracing::error!("Video pipeline thread is dead. Aborting.");
            break;
        }
    }

    tracing::warn!("Got kill signal. Cleaning up.");
    drop(vpp_out); // = signal exit
    vpp_thread.join().unwrap();

    tracing::warn!("Cleanup done. Exiting.");
    Ok(())
}

