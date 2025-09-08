use anyhow::anyhow;
use base64::{engine::general_purpose::STANDARD as base64, Engine};
use crypto_box::aead::rand_core::CryptoRngCore;
use crypto_box::aead::{AeadCore, OsRng};
use crypto_box::{aead::Aead, PublicKey, SalsaBox, SecretKey};
use curve25519_dalek::edwards::CompressedEdwardsY;
use hmac::{Hmac, Mac};
use libp2p_identity::PeerId;
use sha2::Sha256;
use url::form_urlencoded;

pub fn _encrypt(
    worker_id: &PeerId,
    secret_key: &SecretKey,
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), anyhow::Error> {
    encrypt_with_rng(worker_id, secret_key, plaintext, &mut OsRng)
}

pub fn encrypt_with_rng(
    worker_id: &PeerId,
    secret_key: &SecretKey,
    plaintext: &[u8],
    rng: &mut impl CryptoRngCore,
) -> Result<(Vec<u8>, Vec<u8>), anyhow::Error> {
    let peer_id_decoded = worker_id.to_bytes();
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
    let nonce = SalsaBox::generate_nonce(rng);
    let ciphertext =
        shared_box.encrypt(&nonce, plaintext).map_err(|err| anyhow!("Error {err:?}"))?;
    Ok((ciphertext, nonce.to_vec()))
}

// Generate signature with timestamp as required by Cloudflare's is_timed_hmac_valid_v0 function
// https://developers.cloudflare.com/ruleset-engine/rules-language/functions/#hmac-validation
pub fn timed_hmac(message: &str, secret: &str, timestamp: usize) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{}{}", message, timestamp).as_bytes());
    let digest = mac.finalize().into_bytes();
    let token: String =
        form_urlencoded::byte_serialize(base64.encode(digest.as_slice()).as_bytes()).collect();
    format!("{timestamp}-{token}")
}

pub fn _timed_hmac_now(message: &str, secret: &str) -> String {
    let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs().try_into().unwrap();
    timed_hmac(message, secret, timestamp)
}

#[test]
fn test_hmac_sign() {
    let message = "12D3KooWBwbQFT48cNYGPbDwm8rjasbZkc1VMo6rCR6217qr165S";
    let secret = "test_secret";
    let timestamp = 1715662737;
    let expected = "1715662737-E%2BaW1Y5hS587YGeJFKGTnp%2Fhn8rEMmSRlEslPiOQsuE%3D";
    assert_eq!(timed_hmac(message, secret, timestamp), expected);
}
