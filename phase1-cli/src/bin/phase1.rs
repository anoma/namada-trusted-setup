use phase1_coordinator::{
    authentication::{KeyPair, Production, Signature},
    commands::{Computation, RandomSource, SEED_LENGTH},
    io,
    objects::{ContributionFileSignature, ContributionInfo, ContributionState},
    rest::{ContributorStatus, PostChunkRequest, UPDATE_TIME},
    storage::Object,
};

use reqwest::{Client, Url};

use anyhow::Result;
use async_stream::try_stream;
use crossterm::{
    execute,
    terminal::{Clear, ClearType, ScrollDown},
};
use futures_util::StreamExt;
use phase1_cli::{requests, CeremonyOpt, keys::StoredKeypair};
use serde::Serialize;
use serde_json;
use setup_utils::calculate_hash;
use structopt::StructOpt;

use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    sync::Arc,
};

use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

use regex::Regex;

use tokio::{fs as async_fs, io::AsyncWriteExt, task::JoinHandle, time};
use tokio_util::io::ReaderStream;

use tracing::{debug, trace};

const OFFLINE_CONTRIBUTION_FILE_NAME: &str = "contribution.params";
const OFFLINE_CHALLENGE_FILE_NAME: &str = "challenge.params";

macro_rules! pretty_hash {
    ($hash:expr) => {{
        let mut output = format!("\n\n");
        for line in $hash.chunks(16) {
            output += "\t";
            for section in line.chunks(4) {
                for b in section {
                    output += &format!("{:02x}", b);
                }
                output += " ";
            }
            output += "\n";
        }
        output
    }};
}

/// Asks the user a few questions to properly setup the contribution
#[inline(always)]
fn initialize_contribution() -> Result<ContributionInfo> {
    let mut contrib_info = ContributionInfo::default();
    let incentivization = io::get_user_input(
        "Do you want to participate in the incentivised trusted setup? [y/n]".yellow(),
        Some(&Regex::new(r"^(?i)[yn]$")?),
    )?
    .to_lowercase();

    if incentivization == "y" {
        // Ask for personal info
        contrib_info.full_name = Some(io::get_user_input("Please enter your full name:".yellow(), None)?);
        contrib_info.email = Some(io::get_user_input(
            "Please enter your email address:".yellow(),
            Some(&Regex::new(r".+[@].+[.].+")?),
        )?);
        contrib_info.is_incentivized = true;
    };

    Ok(contrib_info)
}

/// Asks the user wheter he wants to use a custom seed of randomness or not
fn get_seed_of_randomness() -> Result<bool> {
    let custom_seed = io::get_user_input(
        "Do you want to input your own seed of randomness (32 bytes hex encoded)? [y/n]".yellow(),
        Some(&Regex::new(r"^(?i)[yn]$")?),
    )?
    .to_lowercase();

    if custom_seed == "y" {
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Prompt the user with the second round of questions to define which execution branch to follow
#[inline(always)]
fn get_contribution_branch(mut contrib_info: ContributionInfo) -> Result<ContributionInfo> {
    let offline = io::get_user_input(
        "Do you want to contribute on another machine? [y/n]".yellow(),
        Some(&Regex::new(r"^(?i)[yn]$")?),
    )?
    .to_lowercase();

    if offline == "y" {
        contrib_info.is_another_machine = true;
    } else {
        if get_seed_of_randomness()? {
            contrib_info.is_own_seed_of_randomness = true;
        }
    }

    Ok(contrib_info)
}

#[inline(always)]
fn get_file_as_byte_vec(filename: &str, round_height: u64, contribution_id: u64) -> Result<Vec<u8>> {
    let mut f = File::open(filename)?;
    let metadata = fs::metadata(filename)?;

    let anoma_file_size: u64 = Object::anoma_contribution_file_size(round_height, contribution_id);
    let mut buffer = vec![0; anoma_file_size as usize];
    debug!(
        "namada_contribution_file_size: round_height {}, contribution_id {}",
        round_height, contribution_id
    );
    debug!("metadata file length {}", metadata.len());
    f.read(&mut buffer)?;

    Ok(buffer)
}

fn get_progress_bar(len: u64) -> ProgressBar {
    let progress_bar = ProgressBar::new(len);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {bytes_per_sec} {total_bytes}")
            .progress_chars("#>-"),
    );

    progress_bar
}

/// Contest and offline execution branches
#[inline(always)]
fn compute_contribution_offline() -> Result<()> {
    // Print instructions to the user
    let mut msg = format!(
        "{}:\n
        In the current working directory, you can find the challenge file \"{}\" and contribution file \"{}\".
        To contribute, you will need both files. Use the contribution file as prelude and append your contribution to it.
        Starting from now, you will have 15 minutes of time to compute randomness and upload your contribution, after which you will be dropped out of the ceremony.\n",
        "Instructions".bold().underline(),
        OFFLINE_CONTRIBUTION_FILE_NAME,
        OFFLINE_CHALLENGE_FILE_NAME
    );
    msg.push_str("\nIf you want to use the provided \"contribute --offline\" command follow these steps:\n");
    msg.push_str(
    format!(
        "{:4}{}- Copy both the challenge file \"{}\" and contribution file \"{}\" in the directory where you will execute the offline command\n",
        "", "1".bold(), 
        OFFLINE_CHALLENGE_FILE_NAME,
        OFFLINE_CONTRIBUTION_FILE_NAME
    ).as_str());
    msg.push_str(
        format!(
            "{:4}{}- Execute the command \"{}\"\n",
            "",
            "2".bold(),
            "cargo run --release --bin phase1 --features=cli contribute --offline".bold()
        )
        .as_str(),
    );
    msg.push_str(
        format!(
            "{:4}{}- Copy the contribution file \"{}\" back to this directory (by overwriting the previousvicious file)",
            "",
            "3".bold(),
            OFFLINE_CONTRIBUTION_FILE_NAME
        )
        .as_str(),
    );
    println!("{}", msg);

    // Wait for the contribution file to be updated with randomness
    // NOTE: we don't actually check for the timeout on the 15 minutes. If the user takes more time than allowed to produce the file we'll keep going on in the contribution, at the following request the Coordinator will reply with an error because ther contributor has been dropped out of the ceremony
    io::get_user_input("When the contribution file is ready, press enter to upload it and move on".yellow(), None)?;

    Ok(())
}

/// Computes randomness
fn compute_contribution(custom_seed: bool, challenge: &[u8], filename: &str) -> Result<()> {
    let rand_source = if custom_seed {
        let seed_str = io::get_user_input(
            "Enter your own seed of randomness, 32 bytes hex encoded".yellow(),
            Some(&Regex::new(r"[[:xdigit:]]{64}")?),
        )?;
        let mut seed = [0u8; SEED_LENGTH];

        for (i, val) in hex::decode(seed_str)?.into_iter().enumerate() {
            seed[i] = val;
        }
        RandomSource::Seed(seed)
    } else {
        let entropy = io::get_user_input("Enter a random string to be used as entropy:".yellow(), None)?;
        RandomSource::Entropy(entropy)
    };

    let writer = OpenOptions::new().append(true).open(filename)?;

    #[cfg(debug_assertions)]
    Computation::contribute_test_masp(challenge, writer, &rand_source);
    #[cfg(not(debug_assertions))]
    Computation::contribute_masp(challenge, writer, &rand_source);

    println!(
        "{}",
        "Randomness has been correctly produced in the target file"
            .green()
            .bold()
    );
    Ok(())
}

/// Performs the contribution sequence. Returns the round height of the contribution.
#[inline(always)]
async fn contribute(
    client: &Client,
    coordinator: &Url,
    keypair: &KeyPair,
    mut contrib_info: ContributionInfo,
    heartbeat_handle: &JoinHandle<()>,
) -> Result<u64> {
    // Get the necessary info to compute the contribution
    println!("{} Locking chunk", "[4/11]".bold().dimmed());
    let locked_locators = requests::get_lock_chunk(client, coordinator, keypair).await?;
    contrib_info.timestamps.challenge_locked = Utc::now();
    let response_locator = locked_locators.next_contribution();
    let round_height = response_locator.round_height();
    contrib_info.ceremony_round = round_height;

    let challenge_url = requests::get_challenge_url(client, coordinator, keypair, &round_height).await?;
    debug!("Presigned url: {}", challenge_url);
    println!("{} Getting challenge", "[5/11]".bold().dimmed());
    let mut challenge_stream = requests::get_challenge(client, challenge_url.as_str()).await?;
    let progress_bar = get_progress_bar(challenge_stream.1);
    let mut challenge: Vec<u8> = Vec::new();
    while let Some(b) = challenge_stream.0.next().await {
        let b = b?;
        challenge.extend_from_slice(&b);
        progress_bar.inc(b.len() as u64);
    }
    progress_bar.finish();
    contrib_info.timestamps.challenge_downloaded = Utc::now();

    // Saves the challenge locally, in case the contributor is paranoid and wants to double check himself. It is also used in the offline contrib path
    contrib_info = tokio::task::spawn_blocking(move || get_contribution_branch(contrib_info)).await??;
    let challenge_filename = if contrib_info.is_another_machine {
        OFFLINE_CHALLENGE_FILE_NAME.to_string()
    } else {
        format!("namada_challenge_round_{}.params", round_height)
    };
    let mut challenge_writer = async_fs::File::create(challenge_filename.as_str()).await?;
    challenge_writer.write_all(&challenge.as_slice()).await?;

    let challenge_hash = calculate_hash(challenge.as_ref());
    debug!("Challenge hash is {}", pretty_hash!(&challenge_hash));
    debug!("Challenge length {}", challenge.len());

    // Prepare contribution file with the challege hash
    println!("{} Setting up contribution file", "[6/11]".bold().dimmed());
    let contrib_filename = if contrib_info.is_another_machine {
        Arc::new(OFFLINE_CONTRIBUTION_FILE_NAME.to_string())
    } else {
        Arc::new(format!(
        "namada_contribution_round_{}_public_key_{}.params",
        round_height, keypair.pubkey()
    ))
};
    let mut response_writer = async_fs::File::create(contrib_filename.as_str()).await?;
    response_writer.write_all(challenge_hash.to_vec().as_ref()).await?;

    // Compute contribution
    println!("{} Computing contribution", "[7/11]".bold().dimmed());

    let contrib_filename_copy = contrib_filename.clone();
    contrib_info.timestamps.start_computation = Utc::now();
    if contrib_info.is_another_machine {
        tokio::task::spawn_blocking(move || compute_contribution_offline()).await??;
    } else {
        let custom_seed = contrib_info.is_own_seed_of_randomness;
        tokio::task::spawn_blocking(move || {
            compute_contribution(custom_seed, challenge.as_ref(), contrib_filename_copy.as_str())
        })
        .await??;
    }
    let contrib_filename_copy = contrib_filename.clone();
    let contribution = tokio::task::spawn_blocking(move || {
        get_file_as_byte_vec(
            contrib_filename_copy.as_str(),
            round_height,
            response_locator.contribution_id(),
        )
    })
    .await??;

    contrib_info.timestamps.end_computation = Utc::now();
    trace!("Response writer {:?}", response_writer);
    println!(
        "{}",
        format!(
            "Completed contribution in {} seconds",
            (contrib_info.timestamps.end_computation - contrib_info.timestamps.start_computation).num_seconds()
        )
        .green()
        .bold()
    );

    // Update contribution info
    println!("{} Updating contribution info", "[8/11]".bold().dimmed());
    let contribution_file_hash = calculate_hash(contribution.as_ref());
    let contribution_file_hash_str = hex::encode(contribution_file_hash);
    debug!("Contribution hash is {}", contribution_file_hash_str);
    debug!("Contribution length: {}", contribution.len());
    contrib_info.contribution_file_hash = contribution_file_hash_str;
    contrib_info.contribution_file_signature =
        Production.sign(keypair.sigkey(), contrib_info.contribution_file_hash.as_str())?;
    let challenge_hash_len = challenge_hash.len();
    contrib_info.contribution_hash = hex::encode(calculate_hash(&contribution[challenge_hash_len..]));
    contrib_info.contribution_hash_signature =
        Production.sign(keypair.sigkey(), contrib_info.contribution_hash.as_str())?;

    // Send contribution to the coordinator
    let contribution_state = ContributionState::new(challenge_hash.to_vec(), contribution_file_hash.to_vec(), None)?;

    let signature = Production.sign(keypair.sigkey(), &contribution_state.signature_message()?)?;
    let contribution_file_signature = ContributionFileSignature::new(signature, contribution_state)?;

    let (contribution_url, contribution_signature_url) =
        requests::get_contribution_url(client, coordinator, keypair, &round_height).await?;
    println!("{} Uploading contribution", "[9/11]".bold().dimmed());
    let contrib_file = async_fs::File::open(contrib_filename.as_str()).await?;
    let contrib_size = async_fs::metadata(contrib_filename.as_str()).await?.len();
    let mut stream = ReaderStream::new(contrib_file);
    let pb = get_progress_bar(contrib_size);
    let pb_clone = pb.clone();

    let contrib_stream = try_stream! {
        while let Some(b) = stream.next().await {
            let b = b?;
            pb.inc(b.len() as u64);
            yield b;
        }
    };

    requests::upload_chunk(
        client,
        contribution_url.as_str(),
        contribution_signature_url.as_str(),
        contrib_stream,
        contrib_size,
        &contribution_file_signature,
    )
    .await?;
    pb_clone.finish();
    contrib_info.timestamps.end_contribution = Utc::now();

    // Compute signature of contributor info
    contrib_info
        .try_sign(keypair)
        .expect(&format!("{}", "Error while signing the contribution info".red().bold()));

    // Write contribution info file and send it to the Coordinator
    println!("{} Uploading contribution info", "[10/11]".bold().dimmed());
    async_fs::write(
        format!("namada_contributor_info_round_{}.json", contrib_info.ceremony_round),
        &serde_json::to_vec(&contrib_info)?,
    )
    .await?;
    requests::post_contribution_info(client, coordinator, keypair, &contrib_info).await?;

    // Notify contribution to the coordinator for the verification
    println!(
        "{} Notifying the coordinator of the completed contribution",
        "[11/11]".bold().dimmed()
    );
    let post_chunk_req = PostChunkRequest::new(
        round_height,
        locked_locators.next_contribution(),
        locked_locators.next_contribution_file_signature(),
    );
    requests::post_contribute_chunk(client, coordinator, keypair, &post_chunk_req).await?;

    // Interrupt heartbeat, to prevent heartbeating during verification
    // NOTE: need to manually cancel the heartbeat task because, by default, async runtimes use detach on drop strategy
    //  (see https://blog.yoshuawuyts.com/async-cancellation-1/#cancelling-tasks), meaning that the task
    //  only gets detached from the main execution unit but keeps running in the background until the main
    //  function returns. This would cause the contributor to send heartbeats even after it has been removed
    //  from the list of current contributors, causing an error
    heartbeat_handle.abort();

    Ok(round_height)
}

/// Waits in line until it's time to contribute
#[inline(always)]
async fn contribution_loop(
    client: Arc<Client>,
    coordinator: Arc<Url>,
    keypair: Arc<KeyPair>,
    mut contrib_info: ContributionInfo,
) {
    println!("{} Joining queue", "[3/11]".bold().dimmed());
    requests::post_join_queue(&client, &coordinator, &keypair)
        .await
        .expect(&format!("{}", "Couldn't join the queue".red().bold()));
    contrib_info.timestamps.joined_queue = Utc::now();

    // Spawn heartbeat task to prevent the Coordinator from
    // dropping the contributor out of the ceremony in the middle of a contribution.
    // Heartbeat is checked by the Coordinator every 120 seconds.
    let client_cnt = client.clone();
    let coordinator_cnt = coordinator.clone();
    let keypair_cnt = keypair.clone();

    let heartbeat_handle = tokio::task::spawn(async move {
        loop {
            if let Err(e) = requests::post_heartbeat(&client_cnt, &coordinator_cnt, &keypair_cnt).await {
                eprintln!(
                    "{}",
                    format!("{}: {}", "Heartbeat error".red().bold(), e.to_string().red().bold())
                );
            }
            time::sleep(UPDATE_TIME).await;
        }
    });

    let mut round_height = 0;
    let mut status_count = 1;

    loop {
        // Check the contributor's position in the queue
        let queue_status = requests::get_contributor_queue_status(&client, &coordinator, &keypair)
            .await
            .expect(&format!("{}", "Couldn't get the status of contributor".red().bold()));

        match queue_status {
            ContributorStatus::Queue(position, size) => {
                let msg = format!(
                    "Queue position: {}\nQueue size: {}\nEstimated waiting time: {} min",
                    position,
                    size,
                    position * 5
                );

                let max_len = msg.split("\n").map(|x| x.len()).max().unwrap();
                let stripe = "=".repeat(max_len);

                if status_count > 1 {
                    // Clear previous status from terminal
                    execute!(std::io::stdout(), ScrollDown(6), Clear(ClearType::FromCursorDown)).unwrap();
                }
                println!(
                    "{}{}\n{}\n{}\n{}",
                    "Queue status, poll #", status_count, stripe, msg, stripe
                );
                status_count += 1;
            }
            ContributorStatus::Round => {
                round_height = contribute(&client, &coordinator, &keypair, contrib_info.clone(), &heartbeat_handle)
                    .await
                    .expect(&format!("{}", "Contribution failed".red().bold()));
            }
            ContributorStatus::Finished => {
                println!(
                    "{} \nNext, your contribution will be verified by the coordinator.
                    You can check the outcome in a few minutes by looking at round height {}.
                    If you don't see it or the public key doesn't match yours, it means your contribution didn't pass the verification step.",
                    "Done! Thank you for your contribution!".green().bold(),
                    round_height
                );
                break;
            }
            ContributorStatus::Banned => {
                println!(
                    "{}",
                    "This contributor has been banned from the ceremony because of an invalid contribution."
                        .red()
                        .bold()
                );
                break;
            }
            ContributorStatus::Other => {
                println!("{}", "Did not retrieve e valid contributor state.".red().bold());
                break;
            }
        }

        // Get status updates
        time::sleep(UPDATE_TIME).await;
    }
}

#[inline(always)]
async fn close_ceremony(client: &Client, coordinator: &Url, keypair: &KeyPair) {
    match requests::get_stop_coordinator(client, coordinator, keypair).await {
        Ok(()) => println!("{}", "Ceremony completed!".green().bold()),
        Err(e) => eprintln!("{}", e.to_string().red().bold()),
    }
}

#[inline(always)]
async fn get_contributions(coordinator: &Url) {
    match requests::get_contributions_info(coordinator).await {
        Ok(contributions) => {
            let contributions_str = std::str::from_utf8(&contributions).unwrap();
            println!("Contributions:\n{}", contributions_str)
        }
        Err(e) => eprintln!("{}", e.to_string().red().bold()),
    }
}

#[cfg(debug_assertions)]
#[inline(always)]
async fn verify_contributions(client: &Client, coordinator: &Url, keypair: &KeyPair) {
    match requests::get_verify_chunks(client, coordinator, keypair).await {
        Ok(()) => println!("{}", "Verification of pending contributions completed".green().bold()),
        Err(e) => eprintln!("{}", e.to_string().red().bold()),
    }
}

#[cfg(debug_assertions)]
#[inline(always)]
async fn update_coordinator(client: &Client, coordinator: &Url, keypair: &KeyPair) {
    match requests::get_update(client, coordinator, keypair).await {
        Ok(()) => println!("{}", "Coordinator updated".green().bold()),
        Err(e) => eprintln!("{}", e.to_string().red().bold()),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let opt = CeremonyOpt::from_args();

    match opt {
        CeremonyOpt::Contribute { url, offline } => {
            if offline {
                // Only compute randomness. It expects a file called contribution.params to be available in the cwd and already filled with the challenge bytes
                println!("{} Reading challenge", "[1/2]".bold().dimmed());
                let challenge = async_fs::read(OFFLINE_CHALLENGE_FILE_NAME)
                    .await
                    .expect(&format!("{}", "Couldn't read the challenge file".red().bold()));

                println!("{} Computing contribution", "[2/2]".bold().dimmed());
                tokio::task::spawn_blocking(move || {
                    compute_contribution(
                        get_seed_of_randomness().unwrap(),
                        &challenge,
                        OFFLINE_CONTRIBUTION_FILE_NAME,
                    )
                })
                .await
                .unwrap()
                .expect(&format!("{}", "Error in computing randomness".red().bold()));

                return;
            }

            // Perform the entire contribution cycle
            let banner = async_fs::read_to_string("phase1-cli/ascii_logo.txt").await.unwrap();
            println!("{}", banner.cyan());
            println!("{}", "Welcome to the Namada Trusted Setup Ceremony!".bold());
            println!("{} Generating keypair", "[1/11]".bold().dimmed());
            io::get_user_input("Press enter to continue".yellow(), None).unwrap();
            let keypair = tokio::task::spawn_blocking(|| io::generate_keypair(false))
                .await
                .unwrap()
                .expect(&format!("{}", "Error while generating the keypair".red().bold()));

            println!("{} Initializing contribution", "[2/11]".bold().dimmed());
            let mut contrib_info = tokio::task::spawn_blocking(initialize_contribution)
                .await
                .unwrap()
                .expect(&format!("{}", "Error while initializing the contribution".red().bold()));
            contrib_info.timestamps.start_contribution = Utc::now();
            contrib_info.public_key = keypair.pubkey().to_string();

            contribution_loop(
                Arc::new(Client::new()),
                Arc::new(url.coordinator),
                Arc::new(keypair),
                contrib_info,
            )
            .await;
        }
        CeremonyOpt::CloseCeremony(url) => {
            let keypair = tokio::task::spawn_blocking(|| io::keypair_from_mnemonic())
                .await
                .unwrap()
                .expect(&format!("{}", "Error while generating the keypair".red().bold()));

            let client = Client::new();
            close_ceremony(&client, &url.coordinator, &keypair).await;
        }
        CeremonyOpt::ExportKeypair(mnemonic_path) => {
            let content = async_fs::read_to_string(mnemonic_path.path).await.unwrap();
            let seed = io::seed_from_string(content.as_str()).unwrap();

            let password = io::get_user_input("Enter the password to encrypt the keypair. Make sure to safely store this password:".yellow(), None).unwrap();
            // FIXME: hide stdin
            let confirmation = io::get_user_input("Enter again the password to confirm:".yellow(), None).unwrap();
            if confirmation != password {
                eprintln!(
                    "{}",
                    format!("{}", "Passwords don't match!".red().bold())
                );
            }

            let keypair = StoredKeypair::from_seed(&seed, password);
            async_fs::write("keypair", toml::to_vec(&keypair).unwrap()).await.unwrap();
            println!("{}", "Keypair was correctly generated in \"keypair\" file".bold().green());
        }
        CeremonyOpt::GetContributions(url) => {
            get_contributions(&url.coordinator).await;
        }
        #[cfg(debug_assertions)]
        CeremonyOpt::VerifyContributions(url) => {
            let keypair = tokio::task::spawn_blocking(|| io::keypair_from_mnemonic())
                .await
                .unwrap()
                .expect(&format!("{}", "Error while generating the keypair".red().bold()));

            let client = Client::new();
            verify_contributions(&client, &url.coordinator, &keypair).await;
        }
        #[cfg(debug_assertions)]
        CeremonyOpt::UpdateCoordinator(url) => {
            let keypair = tokio::task::spawn_blocking(|| io::keypair_from_mnemonic())
                .await
                .unwrap()
                .expect(&format!("{}", "Error while generating the keypair".red().bold()));

            let client = Client::new();
            update_coordinator(&client, &url.coordinator, &keypair).await;
        }
    }
}
