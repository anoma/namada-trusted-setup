//! REST API endpoints exposed by the [Coordinator](`crate::Coordinator`).

use crate::{
    objects::Task,
    storage::{ContributionLocator, ContributionSignatureLocator},
    ContributionFileSignature,
};
use rocket::{
    error, get,
    http::{ContentType, Status},
    post,
    response::{Responder, Response},
    serde::{json::Json, Deserialize, Serialize},
    Request, Shutdown, State,
};

use crate::{objects::LockedLocators, CoordinatorError, Participant};

use std::{collections::LinkedList, io::Cursor, net::SocketAddr, sync::Arc};
use thiserror::Error;

use tokio::sync::RwLock;

use tracing::debug;

type Coordinator = Arc<RwLock<crate::Coordinator>>;

/// Server errors. Also includes errors generated by the managed [Coordinator](`crate::Coordinator`).
#[derive(Error, Debug)]
pub enum ResponseError {
    #[error("Coordinator failed: {0}")]
    CoordinatorError(CoordinatorError),
    #[error("Error while terminating the ceremony: {0}")]
    ShutdownError(String),
    #[error("Could not find contributor with public key {0}")]
    UnknownContributor(String),
    #[error("Could not find the provided Task {0} in coordinator state")]
    UnknownTask(Task),
    #[error("Error while verifying a contribution: {0}")]
    VerificationError(String),
}

impl<'r> Responder<'r, 'static> for ResponseError {
    fn respond_to(self, _request: &'r Request<'_>) -> rocket::response::Result<'static> {
        let response = format!("{}", self);
        Response::build()
            .status(Status::InternalServerError)
            .header(ContentType::JSON)
            .sized_body(response.len(), Cursor::new(response))
            .ok()
    }
}

type Result<T> = std::result::Result<T, ResponseError>;

/// Request to get a [Chunk](`crate::objects::Chunk`).
#[derive(Deserialize, Serialize)]
pub struct GetChunkRequest {
    pubkey: String,
    locked_locators: LockedLocators,
}

impl GetChunkRequest {
    pub fn new(pubkey: String, locked_locators: LockedLocators) -> Self {
        GetChunkRequest {
            pubkey,
            locked_locators,
        }
    }
}

/// Contribution of a [Chunk](`crate::objects::Chunk`).
#[derive(Deserialize, Serialize)]
pub struct ContributeChunkRequest {
    pubkey: String,
    chunk_id: u64,
}

impl ContributeChunkRequest {
    pub fn new(pubkey: String, chunk_id: u64) -> Self {
        Self { pubkey, chunk_id }
    }
}

/// Request to post a [Chunk](`crate::objects::Chunk`).
#[derive(Deserialize, Serialize)]
pub struct PostChunkRequest {
    contribution_locator: ContributionLocator,
    contribution: Vec<u8>,
    contribution_file_signature_locator: ContributionSignatureLocator,
    contribution_file_signature: ContributionFileSignature,
}

impl PostChunkRequest {
    pub fn new(
        contribution_locator: ContributionLocator,
        contribution: Vec<u8>,
        contribution_file_signature_locator: ContributionSignatureLocator,
        contribution_file_signature: ContributionFileSignature,
    ) -> Self {
        Self {
            contribution_locator,
            contribution,
            contribution_file_signature_locator,
            contribution_file_signature,
        }
    }
}

//
// -- REST API ENDPOINTS --
//

/// Add the incoming contributor to the queue of contributors.
#[post("/contributor/join_queue", format = "json", data = "<contributor_pubkey>")]
pub async fn join_queue(
    coordinator: &State<Coordinator>,
    contributor_pubkey: Json<String>,
    contributor_ip: SocketAddr,
) -> Result<()> {
    let pubkey = contributor_pubkey.into_inner();
    let contributor = Participant::new_contributor(pubkey.as_str());

    match coordinator
        .write()
        .await
        .add_to_queue(contributor, Some(contributor_ip.ip()), 10)
    {
        Ok(()) => Ok(()),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Lock a [Chunk](`crate::objects::Chunk`) in the ceremony. This should be the first function called when attempting to contribute to a chunk. Once the chunk is locked, it is ready to be downloaded.
#[post("/contributor/lock_chunk", format = "json", data = "<contributor_pubkey>")]
pub async fn lock_chunk(
    coordinator: &State<Coordinator>,
    contributor_pubkey: Json<String>,
) -> Result<Json<LockedLocators>> {
    let pubkey = contributor_pubkey.into_inner();
    let contributor = Participant::new_contributor(pubkey.as_str());

    match coordinator.write().await.try_lock(&contributor) {
        Ok((_, locked_locators)) => Ok(Json(locked_locators)),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Download a chunk from the [Coordinator](`crate::Coordinator`), which should be contributed to upon receipt.
#[get("/download/chunk", format = "json", data = "<get_chunk_request>")]
pub async fn get_chunk(
    coordinator: &State<Coordinator>,
    get_chunk_request: Json<GetChunkRequest>,
) -> Result<Json<Task>> {
    let request = get_chunk_request.into_inner();
    let contributor = Participant::new_contributor(request.pubkey.as_ref());

    let next_contribution = request.locked_locators.next_contribution();

    // Build and check next Task
    let task = Task::new(next_contribution.chunk_id(), next_contribution.contribution_id());

    match coordinator.read().await.state().current_participant_info(&contributor) {
        Some(info) => {
            if !info.pending_tasks().contains(&task) {
                return Err(ResponseError::UnknownTask(task));
            }
            Ok(Json(task))
        }
        None => Err(ResponseError::UnknownContributor(request.pubkey)),
    }
}

#[get("/contributor/challenge", format = "json", data = "<locked_locators>")]
pub async fn get_challenge(
    coordinator: &State<Coordinator>,
    locked_locators: Json<LockedLocators>,
) -> Result<Json<Vec<u8>>> {
    let request = locked_locators.into_inner();

    let challenge_locator = request.current_contribution();
    let round_height = challenge_locator.round_height();
    let chunk_id = challenge_locator.chunk_id();

    debug!(
        "rest::get_challenge - round_height {}, chunk_id {}, contribution_id 0, is_verified true",
        round_height, chunk_id
    );
    // Since we don't chunk the parameters, we have one chunk and one allowed contributor per round. Thus the challenge will always be located at round_{i}/chunk_0/contribution_0.verified
    // For example, the 1st challenge (after the initialization) is located at round_1/chunk_0/contribution_0.verified
    match coordinator.write().await.get_challenge(round_height, chunk_id, 0, true) {
        Ok(challenge_hash) => Ok(Json(challenge_hash)),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Upload a [Chunk](`crate::objects::Chunk`) contribution to the [Coordinator](`crate::Coordinator`). Write the contribution bytes to
/// disk at the provided [Locator](`crate::storage::Locator`). Also writes the corresponding [`ContributionFileSignature`]
#[post("/upload/chunk", format = "json", data = "<post_chunk_request>")]
pub async fn post_contribution_chunk(
    coordinator: &State<Coordinator>,
    post_chunk_request: Json<PostChunkRequest>,
) -> Result<()> {
    let request = post_chunk_request.into_inner();

    if let Err(e) = coordinator
        .write()
        .await
        .write_contribution(request.contribution_locator, request.contribution)
    {
        return Err(ResponseError::CoordinatorError(e));
    }

    match coordinator.write().await.write_contribution_file_signature(
        request.contribution_file_signature_locator,
        request.contribution_file_signature,
    ) {
        Ok(()) => Ok(()),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Notify the [Coordinator](`crate::Coordinator`) of a finished and uploaded [Contribution](`crate::objects::Contribution`). This will unlock the given [Chunk](`crate::objects::Chunk`) and allow the contributor to take on a new task.
#[post(
    "/contributor/contribute_chunk",
    format = "json",
    data = "<contribute_chunk_request>"
)]
pub async fn contribute_chunk(
    coordinator: &State<Coordinator>,
    contribute_chunk_request: Json<ContributeChunkRequest>,
) -> Result<Json<ContributionLocator>> {
    let request = contribute_chunk_request.into_inner();
    let contributor = Participant::new_contributor(request.pubkey.as_ref());

    match coordinator.write().await.try_contribute(&contributor, request.chunk_id) {
        Ok(contribution_locator) => Ok(Json(contribution_locator)),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Update the [Coordinator](`crate::Coordinator`) state.
#[get("/update")]
pub async fn update_coordinator(coordinator: &State<Coordinator>) -> Result<()> {
    match coordinator.write().await.update() {
        Ok(()) => Ok(()),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Lets the [Coordinator](`crate::Coordinator`) know that the participant is still alive and participating (or waiting to participate) in the ceremony.
#[post("/contributor/heartbeat", format = "json", data = "<contributor_pubkey>")]
pub async fn heartbeat(coordinator: &State<Coordinator>, contributor_pubkey: Json<String>) -> Result<()> {
    let pubkey = contributor_pubkey.into_inner();
    let contributor = Participant::new_contributor(pubkey.as_str());
    match coordinator.write().await.heartbeat(&contributor) {
        Ok(()) => Ok(()),
        Err(e) => Err(ResponseError::CoordinatorError(e)),
    }
}

/// Get the pending tasks of contributor.
#[get("/contributor/get_tasks_left", format = "json", data = "<contributor_pubkey>")]
pub async fn get_tasks_left(
    coordinator: &State<Coordinator>,
    contributor_pubkey: Json<String>,
) -> Result<Json<LinkedList<Task>>> {
    let pubkey = contributor_pubkey.into_inner();
    let contributor = Participant::new_contributor(pubkey.as_str());

    match coordinator.read().await.state().current_participant_info(&contributor) {
        Some(info) => Ok(Json(info.pending_tasks().to_owned())),
        None => Err(ResponseError::UnknownContributor(pubkey)),
    }
}

/// Stop the [Coordinator](`crate::Coordinator`) and shuts the server down. This endpoint should be accessible only by the coordinator itself.
#[get("/stop")]
pub async fn stop_coordinator(coordinator: &State<Coordinator>, shutdown: Shutdown) -> Result<()> {
    let result = coordinator
        .write()
        .await
        .shutdown()
        .map_err(|e| ResponseError::ShutdownError(format!("{}", e)));

    // Shut Rocket server down
    shutdown.notify();

    result
}

/// Verify all the pending contributions. This endpoint should be accessible only by the coordinator itself.
#[get("/verify")]
pub async fn verify_chunks(coordinator: &State<Coordinator>) -> Result<()> {
    // Get all the pending verifications, loop on each one of them and perform verification
    let pending_verifications = coordinator.read().await.get_pending_verifications().to_owned();

    let mut write_lock = coordinator.write().await;

    for (task, _) in &pending_verifications {
        // NOTE: we are going to rely on the single default verifier built in the coordinator itself,
        //  no external verifiers. If a verification fails return immediately without verifying the remaining contributions
        write_lock
            .default_verify(task)
            .map_err(|e| ResponseError::VerificationError(format!("{}", e)))?;
    }

    Ok(())
}
