//! Signed chain checkpoints `{project, seq, head_hash, hlc_ts}` (spec §14.4, §19.2).
//!
//! Signing sits behind [`CheckpointSigner`] so a hardware-held key (HSM, TPM, platform key
//! store) can replace the software key without touching the chain. The bundled
//! [`EcdsaP384Signer`] is ECDSA P-384 over SHA-384 with RFC 6979 deterministic nonces
//! (RustCrypto `p384`), the §19.2 rule for software keys. Key generation, storage under a
//! hardware-protected KEK and rotation belong to the key store (§19.2) and are not here: the
//! caller supplies the 48-byte secret scalar.

use crate::error::{Result, StoreError};
use crate::events::ChainError;
use crate::jcs;
use p384::ecdsa::signature::{Signer, Verifier};
use p384::ecdsa::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Algorithm identifier of ECDSA P-384 with SHA-384 (FIPS 186-5).
pub const ALG_ECDSA_P384_SHA384: &str = "ecdsa-p384-sha384";

/// A checkpoint is written every this many events (proposed, §14.4), and also at batch end,
/// release, export and close.
pub const CHECKPOINT_EVERY: i64 = 1000;

/// A signed checkpoint over the chain head at `seq`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Project UUID.
    pub project: String,
    /// Chain position covered.
    pub seq: i64,
    /// `hash` of event `seq` (hex).
    pub head_hash: String,
    /// HLC at signing.
    pub hlc_ts: i64,
    /// Signature algorithm.
    pub alg: String,
    /// Signer key identifier.
    pub key_id: String,
    /// Signer public key (SEC1 compressed for ECDSA P-384).
    pub public_key: Vec<u8>,
    /// Signature (r ‖ s, 96 bytes for P-384).
    pub signature: Vec<u8>,
}

impl Checkpoint {
    /// The signed message: JCS of `{head_hash, hlc_ts, project, seq}` (the HLC as a decimal
    /// string, since it exceeds the JCS-safe integer range).
    pub fn message(project: &str, seq: i64, head_hash: &str, hlc_ts: i64) -> Vec<u8> {
        let v = json!({
            "head_hash": head_hash,
            "hlc_ts": hlc_ts.to_string(),
            "project": project,
            "seq": seq,
        });
        // Every member is a string or a small integer, so canonicalisation cannot fail.
        jcs::value_to_jcs(&v).unwrap_or_default().into_bytes()
    }

    /// This checkpoint's signed message.
    pub fn signed_message(&self) -> Vec<u8> {
        Self::message(&self.project, self.seq, &self.head_hash, self.hlc_ts)
    }
}

/// Signs checkpoints. Implementations: [`EcdsaP384Signer`] (software key), and later the
/// platform key store or an HSM.
pub trait CheckpointSigner {
    /// Algorithm identifier, e.g. [`ALG_ECDSA_P384_SHA384`].
    fn algorithm(&self) -> &str;
    /// Stable key identifier (fingerprint of the public key).
    fn key_id(&self) -> String;
    /// The public key bytes stored beside each checkpoint.
    fn public_key(&self) -> Vec<u8>;
    /// Signs `message`.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>>;
}

/// Verifies checkpoint signatures against trusted keys.
pub trait CheckpointVerifier {
    /// Ok if `cp` carries a valid signature by a trusted key.
    fn verify(&self, cp: &Checkpoint) -> std::result::Result<(), ChainError>;
}

/// Key identifier of a P-384 public key: `p384:` + the first 128 bits of SHA-384 over the
/// SEC1 compressed point, in hex.
pub fn p384_key_id(sec1_compressed: &[u8]) -> String {
    format!("p384:{}", &jcs::sha384_hex(sec1_compressed)[..32])
}

/// A software ECDSA P-384 key (RFC 6979 deterministic nonces).
pub struct EcdsaP384Signer {
    key: SigningKey,
}

impl std::fmt::Debug for EcdsaP384Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EcdsaP384Signer")
            .field("key_id", &self.key_id())
            .finish_non_exhaustive()
    }
}

impl EcdsaP384Signer {
    /// From a 48-byte big-endian secret scalar (as held by the key store).
    pub fn from_secret_bytes(secret: &[u8]) -> Result<Self> {
        let key = SigningKey::from_slice(secret)
            .map_err(|e| StoreError::Signing(format!("invalid P-384 secret: {e}")))?;
        Ok(Self { key })
    }

    /// The verifying key.
    pub fn verifying_key(&self) -> VerifyingKey {
        *self.key.verifying_key()
    }
}

impl CheckpointSigner for EcdsaP384Signer {
    fn algorithm(&self) -> &str {
        ALG_ECDSA_P384_SHA384
    }

    fn key_id(&self) -> String {
        p384_key_id(&self.public_key())
    }

    fn public_key(&self) -> Vec<u8> {
        self.key
            .verifying_key()
            .to_sec1_point(true)
            .as_bytes()
            .to_vec()
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        let sig: Signature = self
            .key
            .try_sign(message)
            .map_err(|e| StoreError::Signing(e.to_string()))?;
        Ok(sig.to_bytes().to_vec())
    }
}

/// Trusted P-384 verifying keys by key ID (device keys certified by the institution, pinned
/// keys, §14.1 "Signer trust").
#[derive(Clone, Debug, Default)]
pub struct TrustedKeys {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustedKeys {
    /// An empty trust store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trusts a SEC1-encoded P-384 public key; returns its key ID.
    pub fn trust_sec1(&mut self, sec1: &[u8]) -> Result<String> {
        let vk = VerifyingKey::from_sec1_bytes(sec1)
            .map_err(|e| StoreError::Signing(format!("invalid P-384 public key: {e}")))?;
        let id = p384_key_id(vk.to_sec1_point(true).as_bytes());
        self.keys.insert(id.clone(), vk);
        Ok(id)
    }

    /// Trusts a signer's public key.
    pub fn trust_signer(&mut self, signer: &dyn CheckpointSigner) -> Result<String> {
        self.trust_sec1(&signer.public_key())
    }
}

impl CheckpointVerifier for TrustedKeys {
    fn verify(&self, cp: &Checkpoint) -> std::result::Result<(), ChainError> {
        let bad = || ChainError::BadSignature { seq: cp.seq };
        if cp.alg != ALG_ECDSA_P384_SHA384 {
            return Err(bad());
        }
        let vk = self
            .keys
            .get(&cp.key_id)
            .ok_or_else(|| ChainError::UntrustedSigner {
                seq: cp.seq,
                key_id: cp.key_id.clone(),
            })?;
        if vk.to_sec1_point(true).as_bytes() != cp.public_key.as_slice() {
            return Err(bad());
        }
        let sig = Signature::from_slice(&cp.signature).map_err(|_| bad())?;
        vk.verify(&cp.signed_message(), &sig).map_err(|_| bad())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    #[test]
    fn rfc6979_p384_sha384_vector() {
        // RFC 6979 A.2.6, P-384 with SHA-384, message "sample".
        let x = unhex(
            "6B9D3DAD2E1B8C1C05B19875B6659F4DE23C3B667BF297BA9AA47740787137D896D5724E4C70A825F872C9EA60D2EDF5",
        );
        let s = EcdsaP384Signer::from_secret_bytes(&x).unwrap();
        let sig = s.sign(b"sample").unwrap();
        assert_eq!(
            hex::encode_upper(&sig[..48]),
            "94EDBB92A5ECB8AAD4736E56C691916B3F88140666CE9FA73D64C4EA95AD133C81A648152E44ACF96E36DD1E80FABE46"
        );
        assert_eq!(
            hex::encode_upper(&sig[48..]),
            "99EF4AEB15F178CEA1FE40DB2603138F130E740A19624526203B6351D0A3A94FA329C145786E679E7B82C71A38628AC8"
        );
    }

    #[test]
    fn sign_and_verify_checkpoint() {
        let s = EcdsaP384Signer::from_secret_bytes(&[7u8; 48]).unwrap();
        let msg = Checkpoint::message("p", 5, "ab", 1 << 40);
        let mut cp = Checkpoint {
            project: "p".into(),
            seq: 5,
            head_hash: "ab".into(),
            hlc_ts: 1 << 40,
            alg: s.algorithm().into(),
            key_id: s.key_id(),
            public_key: s.public_key(),
            signature: s.sign(&msg).unwrap(),
        };
        let mut trust = TrustedKeys::new();
        assert!(matches!(
            trust.verify(&cp),
            Err(ChainError::UntrustedSigner { .. })
        ));
        trust.trust_signer(&s).unwrap();
        trust.verify(&cp).unwrap();
        cp.head_hash = "ac".into();
        assert!(matches!(
            trust.verify(&cp),
            Err(ChainError::BadSignature { .. })
        ));
        assert!(EcdsaP384Signer::from_secret_bytes(&[0u8; 48]).is_err());
    }
}
