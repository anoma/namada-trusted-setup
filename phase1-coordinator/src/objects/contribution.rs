use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use url::Url;
use url_serde;

// TODO (howardwu): Change this to match.
// #[derive(Debug, Serialize, Deserialize)]
// #[serde(rename_all = "camelCase")]
// pub struct Contributor {
//     #[serde(flatten)]
//     address: String
// }

// TODO (howardwu): Change this to match.
// #[derive(Debug, Serialize, Deserialize)]
// #[serde(rename_all = "camelCase")]
// pub struct Verifier {
//     address: String
// }

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    contributor_id: Option<String>,
    #[serde(with = "url_serde")]
    contributed_location: Option<Url>,
    verifier_id: Option<String>,
    #[serde(with = "url_serde")]
    verified_location: Option<Url>,
    verified: bool,
}

impl Contribution {
    /// Returns a reference to the contributor ID, if it exists.
    /// Otherwise returns `None`.
    #[inline]
    pub fn get_contributor_id(&self) -> &Option<String> {
        &self.contributor_id
    }

    /// Returns a reference to the contributor location, if it exists.
    /// Otherwise returns `None`.
    #[inline]
    pub fn get_contributed_location(&self) -> &Option<Url> {
        &self.contributed_location
    }

    /// Returns a reference to the verifier ID, if it exists.
    /// Otherwise returns `None`.
    #[inline]
    pub fn get_verifier_id(&self) -> &Option<String> {
        &self.verifier_id
    }

    /// Returns a reference to the verifier location, if it exists.
    /// Otherwise returns `None`.
    #[inline]
    pub fn get_verified_location(&self) -> &Option<Url> {
        &self.verified_location
    }

    /// Returns `true` if the contribution has been verified.
    /// Otherwise returns `false`.
    #[inline]
    pub fn is_verified(&self) -> bool {
        self.verified
    }
}