//! Convert a LilyPad RSA public key (PKIX "PUBLIC KEY" PEM, as emitted by
//! `lilypad cmd/keygen`) into the base64 PKCS#1 RSAPublicKey DER string the
//! client's ServerPublicKey()/CreateParameters expects.
//!
//! Provenance: the client's CreateParameters parses a DER blob by reading two
//! length-prefixed big-integers (modulus, exponent) directly — i.e. a PKCS#1
//! RSAPublicKey SEQUENCE, not a PKIX SubjectPublicKeyInfo. We therefore strip
//! the PKIX wrapper and emit the inner RSAPublicKey DER, base64-encoded.
//! (If the smoke test shows the client wants the full PKIX DER instead, switch
//! `to_pkcs1_der` to `to_public_key_der`; this is the one format detail to
//! confirm against the live client when first enabling rsa mode.)
use base64::Engine;
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::RsaPublicKey;

pub fn client_public_key_string(pem: &str) -> anyhow::Result<String> {
    let key = RsaPublicKey::from_public_key_pem(pem.trim())
        .map_err(|e| anyhow::anyhow!("parse PKIX public PEM: {e}"))?;
    let der = key
        .to_pkcs1_der()
        .map_err(|e| anyhow::anyhow!("encode PKCS#1 DER: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(der.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway RSA-1024 public key (NOT a secret), used only to pin the
    // conversion. EXPECTED == `openssl rsa -pubin -RSAPublicKey_out -outform DER | base64`.
    const PUB_PKIX_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDGSd0MT33jjDTmJ0ejEU9KCakC
yJs3cZ+nW7Z6OPVLPCnWeNfS9tYhc+DS2+vKg8NqPJdTZ/D+ujhnQJK0omLkeyzm
u7fn1azTaJel6QusEX9iNP9LxSiJE62/u/jKXL8jH/d044bKw5o4iBA6w7VbV9Tp
IBhM1tJJ6m8QhPepswIDAQAB
-----END PUBLIC KEY-----";

    const EXPECTED_PKCS1_B64: &str = "MIGJAoGBAMZJ3QxPfeOMNOYnR6MRT0oJqQLImzdxn6dbtno49Us8KdZ419L21iFz4NLb68qDw2o8l1Nn8P66OGdAkrSiYuR7LOa7t+fVrNNol6XpC6wRf2I0/0vFKIkTrb+7+MpcvyMf93TjhsrDmjiIEDrDtVtX1OkgGEzW0knqbxCE96mzAgMBAAE=";

    #[test]
    fn pkix_pem_to_pkcs1_base64() {
        let got = client_public_key_string(PUB_PKIX_PEM).unwrap();
        assert_eq!(got, EXPECTED_PKCS1_B64);
    }

    #[test]
    fn garbage_pem_errors() {
        assert!(client_public_key_string("not a pem").is_err());
    }
}
