use core::str;
use std::collections::HashMap;
#[cfg(feature = "assignment_reader")]
use std::collections::{BTreeMap, VecDeque};

use anyhow::anyhow;
#[cfg(feature = "assignment_writer")]
use crypto_box::aead::{AeadCore, OsRng};
use crypto_box::{aead::Aead, PublicKey, SalsaBox, SecretKey};
#[cfg(feature = "assignment_writer")]
use curve25519_dalek::edwards::CompressedEdwardsY;
#[cfg(feature = "assignment_reader")]
use flate2::read::GzDecoder;
#[cfg(feature = "assignment_writer")]
use log::error;
use serde::{Deserialize, Serialize};
#[cfg(feature = "assignment_reader")]
use serde_json::Value;
use serde_with::base64::Base64;
use serde_with::serde_as;
#[cfg(feature = "assignment_reader")]
use sha2::Digest;
#[cfg(feature = "assignment_reader")]
use sha2::Sha512;
#[cfg(feature = "assignment_reader")]
use sha3::digest::generic_array::GenericArray;

#[cfg(feature = "assignment_writer")]
use base64::{engine::general_purpose::STANDARD as base64, Engine};
#[cfg(feature = "assignment_writer")]
use hmac::{Hmac, Mac};
#[cfg(feature = "assignment_writer")]
use sha2::Sha256;
#[cfg(feature = "assignment_writer")]
use url::form_urlencoded;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub id: String,
    pub base_url: String,
    pub files: HashMap<String, String>,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dataset {
    pub id: String,
    pub base_url: String,
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Headers {
    worker_id: String,
    worker_signature: String,
}

#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EncryptedHeaders {
    #[serde_as(as = "Base64")]
    identity: Vec<u8>,
    #[serde_as(as = "Base64")]
    nonce: Vec<u8>,
    #[serde_as(as = "Base64")]
    ciphertext: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAssignment {
    status: String,
    chunks_deltas: Vec<u64>,
    encrypted_headers: EncryptedHeaders,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    datasets: Vec<Dataset>,
    worker_assignments: HashMap<String, WorkerAssignment>,
    #[serde(skip)]
    chunk_map: Option<HashMap<String, u64>>,
    #[serde(skip)]
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkAssignment {
    pub url: String,
    pub id: String,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkState {
    pub network: String,
    pub assignment: NetworkAssignment,
}

impl Assignment {
    #[cfg(feature = "assignment_writer")]
    pub fn add_chunk(&mut self, chunk: Chunk, dataset_id: String, dataset_url: String) {
        match self.datasets.iter_mut().find(|dataset| dataset.id == dataset_id) {
            Some(dataset) => dataset.chunks.push(chunk),
            None => self.datasets.push(Dataset {
                id: dataset_id,
                base_url: dataset_url,
                chunks: vec![chunk],
            }),
        }
        self.chunk_map = None
    }

    #[cfg(feature = "assignment_reader")]
    pub async fn try_download(
        url: String,
        previous_id: Option<String>,
    ) -> Result<Option<Self>, anyhow::Error> {
        let response_state = reqwest::get(url).await?;
        let network_state: NetworkState = response_state.json().await?;
        if Some(network_state.assignment.id.clone()) == previous_id {
            return Ok(None);
        }
        let assignment_url = network_state.assignment.url;
        let response_assignment = reqwest::get(assignment_url).await?;
        let compressed_assignment = response_assignment.bytes().await?;
        let decoder = GzDecoder::new(&compressed_assignment[..]);
        let mut result: Self = serde_json::from_reader(decoder)?;
        result.id = network_state.assignment.id;
        Ok(Some(result))
    }

    #[cfg(feature = "assignment_writer")]
    pub fn insert_assignment(&mut self, peer_id: &str, status: String, chunks_deltas: Vec<u64>) {
        self.worker_assignments.insert(
            peer_id.to_string(),
            WorkerAssignment {
                status,
                chunks_deltas,
                encrypted_headers: Default::default(),
            },
        );
    }

    #[cfg(feature = "assignment_reader")]
    pub fn dataset_chunks_for_peer_id(&self, peer_id: &str) -> Option<Vec<Dataset>> {
        let local_assignment = self.worker_assignments.get(peer_id)?;
        let mut result: Vec<Dataset> = Default::default();
        let mut idxs: VecDeque<u64> = Default::default();
        let mut cursor = 0;
        for v in &local_assignment.chunks_deltas {
            cursor += v;
            idxs.push_back(cursor);
        }
        cursor = 0;
        for u in &self.datasets {
            if idxs.is_empty() {
                break;
            }
            let mut filtered_chunks: Vec<Chunk> = Default::default();
            for v in &u.chunks {
                if idxs[0] < cursor {
                    return None; // Malformed diffs
                }
                if idxs[0] == cursor {
                    filtered_chunks.push(v.clone());
                    idxs.pop_front();
                }
                if idxs.is_empty() {
                    break;
                }
                cursor += 1;
            }
            if !filtered_chunks.is_empty() {
                result.push(Dataset {
                    id: u.id.clone(),
                    base_url: u.base_url.clone(),
                    chunks: filtered_chunks,
                });
            }
        }
        Some(result)
    }

    #[cfg(feature = "assignment_reader")]
    pub fn headers_for_peer_id(
        &self,
        peer_id: &str,
        secret_key: &Vec<u8>,
    ) -> Result<BTreeMap<String, String>, anyhow::Error> {
        let Some(local_assignment) = self.worker_assignments.get(peer_id) else {
            return Err(anyhow!("Can not find assignment for {peer_id}"));
        };
        let EncryptedHeaders {
            identity,
            nonce,
            ciphertext,
        } = &local_assignment.encrypted_headers;
        let temporary_public_key = PublicKey::from_slice(identity.as_slice())?;
        let big_slice = Sha512::default().chain_update(secret_key).finalize();
        let worker_secret_key = SecretKey::from_slice(&big_slice[00..32])?;
        let shared_box = SalsaBox::new(&temporary_public_key, &worker_secret_key);
        let generic_nonce = GenericArray::clone_from_slice(nonce);
        let Ok(decrypted_plaintext) = shared_box.decrypt(&generic_nonce, &ciphertext[..]) else {
            return Err(anyhow!("Can not decrypt payload"));
        };
        let plaintext_headers = std::str::from_utf8(&decrypted_plaintext)?;
        let headers = serde_json::from_str::<Value>(plaintext_headers)?;
        let mut result: BTreeMap<String, String> = Default::default();
        let Some(headers_dict) = headers.as_object() else {
            return Err(anyhow!("Can not parse encrypted map"));
        };
        for (k, v) in headers_dict {
            result.insert(k.to_string(), v.as_str().unwrap().to_string());
        }
        Ok(result)
    }

    #[cfg(feature = "assignment_writer")]
    pub fn chunk_index(&mut self, chunk_id: String) -> Option<u64> {
        if self.chunk_map.is_none() {
            let mut chunk_map: HashMap<String, u64> = Default::default();
            let mut idx = 0;
            for dataset in &self.datasets {
                for chunk in &dataset.chunks {
                    chunk_map.insert(chunk.id.clone(), idx);
                    idx += 1;
                }
            }
            self.chunk_map = Some(chunk_map);
        };
        self.chunk_map.as_ref().unwrap().get(&chunk_id).cloned()
    }

    #[cfg(feature = "assignment_writer")]
    fn encrypt(
        worker_id: &String,
        secret_key: &SecretKey,
        plaintext: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), anyhow::Error> {
        let peer_id_decoded = bs58::decode(worker_id).into_vec()?;
        if peer_id_decoded.len() != 38 {
            return Err(anyhow!("WorkerID parsing failed"));
        }
        let pub_key_edvards_bytes = &peer_id_decoded[6..];
        let public_edvards_compressed = CompressedEdwardsY::from_slice(pub_key_edvards_bytes)?;
        let public_edvards = public_edvards_compressed
            .decompress()
            .ok_or(anyhow!("Failed to decompress Edwards point"))?;
        let public_montgomery = public_edvards.to_montgomery();
        let worker_public_key = PublicKey::from(public_montgomery);

        let shared_box = SalsaBox::new(&worker_public_key, secret_key);
        let nonce = SalsaBox::generate_nonce(&mut OsRng);
        let ciphertext = shared_box
            .encrypt(&nonce, plaintext)
            .map_err(|err| anyhow!("Error {err:?}"))?;
        Ok((ciphertext, nonce.to_vec()))
    }

    #[cfg(feature = "assignment_writer")]
    pub fn regenerate_headers(&mut self, cloudflare_storage_secret: &str) {
        let temporary_secret_key = SecretKey::generate(&mut OsRng);
        let temporary_public_key_bytes = *temporary_secret_key.public_key().as_bytes();

        for (worker_id, worker_assignment) in &mut self.worker_assignments {
            let worker_signature = timed_hmac_now(worker_id, cloudflare_storage_secret);

            let headers = Headers {
                worker_id: worker_id.to_string(),
                worker_signature,
            };
            let plaintext = serde_json::to_vec(&headers).unwrap();
            let (ciphertext, nonce) =
                match Self::encrypt(worker_id, &temporary_secret_key, &plaintext) {
                    Ok(val) => val,
                    Err(err) => {
                        error!("Error while processing headers for {worker_id}: {err:?}");
                        continue;
                    }
                };

            worker_assignment.encrypted_headers = EncryptedHeaders {
                identity: temporary_public_key_bytes.to_vec(),
                nonce,
                ciphertext,
            };
        }
    }
}

// Generate signature with timestamp as required by Cloudflare's is_timed_hmac_valid_v0 function
// https://developers.cloudflare.com/ruleset-engine/rules-language/functions/#hmac-validation
#[cfg(feature = "assignment_writer")]
pub fn timed_hmac(message: &str, secret: &str, timestamp: usize) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{}{}", message, timestamp).as_bytes());
    let digest = mac.finalize().into_bytes();
    let token: String =
        form_urlencoded::byte_serialize(base64.encode(digest.as_slice()).as_bytes()).collect();
    format!("{timestamp}-{token}")
}

#[cfg(feature = "assignment_writer")]
pub fn timed_hmac_now(message: &str, secret: &str) -> String {
    let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs().try_into().unwrap();
    timed_hmac(message, secret, timestamp)
}

#[cfg(feature = "assignment_writer")]
#[test]
fn test_hmac_sign() {
    let message = "12D3KooWBwbQFT48cNYGPbDwm8rjasbZkc1VMo6rCR6217qr165S";
    let secret = "test_secret";
    let timestamp = 1715662737;
    let expected = "1715662737-E%2BaW1Y5hS587YGeJFKGTnp%2Fhn8rEMmSRlEslPiOQsuE%3D";
    assert_eq!(timed_hmac(message, secret, timestamp), expected);
}

#[cfg(all(feature = "assignment_writer", feature = "assignment_reader"))]
#[cfg(test)]
mod tests {
    use sqd_network_transport::Keypair;

    use super::*;

    #[test]
    fn it_works() {
        let mut assignment: Assignment = Default::default();
        let keypair = Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id().to_base58();
        let private_key = keypair.try_into_ed25519().unwrap().secret();

        assignment.insert_assignment(&peer_id, "Ok".to_owned(), Default::default());
        assignment.regenerate_headers("SUPERSECRET");
        let headers = assignment
            .headers_for_peer_id(&peer_id, &private_key.as_ref().to_vec())
            .unwrap_or_default();
        let decrypted_id = headers.get("worker-id").unwrap();
        assert_eq!(peer_id, decrypted_id.to_owned());
    }
}
