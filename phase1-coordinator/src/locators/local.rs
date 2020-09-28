use crate::{environment::Environment, locators::Locator};

use std::path::Path;
use tracing::warn;

#[derive(Debug)]
pub struct Local;

impl Locator for Local {
    /// Returns the round directory for a given round height from the coordinator.
    fn round_directory(environment: &Environment, round_height: u64) -> String
    where
        Self: Sized,
    {
        match environment {
            Environment::Test(_) => format!("./transcript/test/round_{}", round_height),
            Environment::Development(_) => format!("./transcript/development/round_{}", round_height),
            Environment::Production(_) => format!("./transcript/production/round_{}", round_height),
        }
    }

    /// Initializes the round directory for a given environment and round height.
    fn round_directory_init(environment: &Environment, round_height: u64) {
        // If the path does not exist, attempt to initialize the directory path.
        let path = Self::round_directory(environment, round_height);

        if !Path::new(&path).exists() {
            std::fs::create_dir_all(&path).expect("unable to create the chunk directory");
        }
    }

    /// Returns `true` if the round directory for a given round height exists.
    /// Otherwise, returns `false`.
    fn round_directory_exists(environment: &Environment, round_height: u64) -> bool
    where
        Self: Sized,
    {
        let path = Self::round_directory(environment, round_height);
        Path::new(&path).exists()
    }

    /// Resets the round directory for a given environment and round height.
    fn round_directory_reset(environment: &Environment, round_height: u64) {
        // If this is a test environment, attempt to clear it for the coordinator.
        let directory = Self::round_directory(environment, round_height);
        let path = Path::new(&directory);
        match environment {
            Environment::Test(_) => {
                if Self::round_directory_exists(environment, round_height) {
                    warn!("Coordinator is clearing {:?}", &path);
                    std::fs::remove_dir_all(&path).expect("Unable to reset round directory");
                    warn!("Coordinator cleared {:?}", &path);
                }
            }
            Environment::Development(_) => warn!("Coordinator is attempting to clear {:?} in development mode", &path),
            Environment::Production(_) => warn!("Coordinator is attempting to clear {:?} in production mode", &path),
        }
    }

    /// Returns the chunk directory for a given round height and chunk ID from the coordinator.
    fn chunk_directory(environment: &Environment, round_height: u64, chunk_id: u64) -> String
    where
        Self: Sized,
    {
        // Create the transcript directory path.
        let path = Self::round_directory(environment, round_height);

        // Create the chunk directory as `{round_directory}/chunk_{chunk_id}`.
        format!("{}/chunk_{}", path, chunk_id)
    }

    /// Initializes the chunk directory for a given environment, round height, and chunk ID.
    fn chunk_directory_init(environment: &Environment, round_height: u64, chunk_id: u64) {
        // If the path does not exist, attempt to initialize the directory path.
        let path = Self::chunk_directory(environment, round_height, chunk_id);

        if !Path::new(&path).exists() {
            std::fs::create_dir_all(&path).expect("unable to create the chunk directory");
        }
    }

    /// Returns `true` if the chunk directory for a given round height and chunk ID exists.
    /// Otherwise, returns `false`.
    fn chunk_directory_exists(environment: &Environment, round_height: u64, chunk_id: u64) -> bool
    where
        Self: Sized,
    {
        let path = Self::chunk_directory(environment, round_height, chunk_id);
        Path::new(&path).exists()
    }

    /// Returns the contribution locator for a given round, chunk ID, and
    /// contribution ID from the coordinator.
    fn contribution_locator(environment: &Environment, round_height: u64, chunk_id: u64, contribution_id: u64) -> String
    where
        Self: Sized,
    {
        // Fetch the chunk directory path.
        let path = Self::chunk_directory(environment, round_height, chunk_id);

        // Set the contribution locator as `{chunk_directory}/contribution_{contribution_id}`.
        format!("{}/contribution_{}", path, contribution_id)
    }

    /// Initializes the contribution locator file for a given round, chunk ID, and
    /// contribution ID from the coordinator.
    fn contribution_locator_init(environment: &Environment, round_height: u64, chunk_id: u64, contribution_id: u64) {
        // If the path does not exist, attempt to initialize the file path.
        let path = Self::contribution_locator(environment, round_height, chunk_id, contribution_id);

        let directory = Path::new(&path).parent().expect("unable to create parent directory");
        if !directory.exists() {
            std::fs::create_dir_all(&path).expect("unable to create the contribution directory");
        }
    }

    /// Returns `true` if the contribution locator for a given round height, chunk ID,
    /// and contribution ID exists. Otherwise, returns `false`.
    fn contribution_locator_exists(
        environment: &Environment,
        round_height: u64,
        chunk_id: u64,
        contribution_id: u64,
    ) -> bool
    where
        Self: Sized,
    {
        let path = Self::contribution_locator(environment, round_height, chunk_id, contribution_id);
        Path::new(&path).exists()
    }

    /// Returns the round locator for a given round from the coordinator.
    fn round_locator(environment: &Environment, round_height: u64) -> String
    where
        Self: Sized,
    {
        // Create the transcript directory path.
        let path = Self::round_directory(environment, round_height);

        // Create the round locator located at `{round_directory}/output`.
        format!("{}/output", path)
    }

    /// Returns `true` if the round locator for a given round height exists.
    /// Otherwise, returns `false`.
    fn round_locator_exists(environment: &Environment, round_height: u64) -> bool
    where
        Self: Sized,
    {
        let path = Self::round_locator(environment, round_height);
        Path::new(&path).exists()
    }
}