//! Kleiner Test-Authenticator: ein Passkey im Speicher, der wie ein Gerät auf die Optionen des
//! Servers antwortet (Registrierung ohne Attestation, Anmeldung mit ES256-Signatur). So lässt
//! sich der ganze Ablauf einschließlich der Prüfung auf dem Server ohne echtes Gerät testen.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ciborium::Value as Cbor;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub struct SoftAuthenticator {
    /// Herkunft, die der „Browser“ in die Client-Daten schreibt.
    pub origin: String,
    rp_id: String,
    key: SigningKey,
    credential_id: Vec<u8>,
    user_handle: Option<Vec<u8>>,
    counter: u32,
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

impl SoftAuthenticator {
    pub fn new(origin: &str, rp_id: &str) -> Self {
        let mut credential_id = [0u8; 32];
        getrandom::fill(&mut credential_id).expect("Zufall");
        let key = loop {
            let mut secret = [0u8; 32];
            getrandom::fill(&mut secret).expect("Zufall");
            if let Ok(key) = SigningKey::from_slice(&secret) {
                break key;
            }
        };
        Self {
            origin: origin.into(),
            rp_id: rp_id.into(),
            key,
            credential_id: credential_id.to_vec(),
            user_handle: None,
            counter: 0,
        }
    }

    /// Antwort auf `navigator.credentials.create()` (`options` ist die Antwort des Servers).
    pub fn register(&mut self, options: &Value) -> Value {
        let public_key = &options["publicKey"];
        let challenge = public_key["challenge"].as_str().expect("challenge");
        self.user_handle = Some(unb64(public_key["user"]["id"].as_str().expect("user.id")));

        let client_data = json!({
            "type": "webauthn.create",
            "challenge": challenge,
            "origin": self.origin,
            "crossOrigin": false,
        })
        .to_string();

        let point = self.key.verifying_key().to_sec1_point(false);
        let cose_key = Cbor::Map(vec![
            (Cbor::Integer(1.into()), Cbor::Integer(2.into())),
            (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
            (Cbor::Integer((-1).into()), Cbor::Integer(1.into())),
            (
                Cbor::Integer((-2).into()),
                Cbor::Bytes(point.x().expect("x").to_vec()),
            ),
            (
                Cbor::Integer((-3).into()),
                Cbor::Bytes(point.y().expect("y").to_vec()),
            ),
        ]);
        let mut cose_bytes = Vec::new();
        ciborium::into_writer(&cose_key, &mut cose_bytes).expect("CBOR");

        // rpIdHash, Flags (Anwesenheit, Verifizierung, Credential-Daten), Zähler 0,
        // dann aaguid (nur Nullen), Länge der ID, ID und öffentlicher Schlüssel.
        let mut auth_data = Sha256::digest(self.rp_id.as_bytes()).to_vec();
        auth_data.push(0x01 | 0x04 | 0x40);
        auth_data.extend_from_slice(&0u32.to_be_bytes());
        auth_data.extend_from_slice(&[0u8; 16]);
        auth_data.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
        auth_data.extend_from_slice(&self.credential_id);
        auth_data.extend_from_slice(&cose_bytes);

        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), Cbor::Bytes(auth_data)),
        ]);
        let mut attestation_bytes = Vec::new();
        ciborium::into_writer(&attestation, &mut attestation_bytes).expect("CBOR");

        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": {
                "attestationObject": b64(&attestation_bytes),
                "clientDataJSON": b64(client_data.as_bytes()),
            },
            "clientExtensionResults": {},
        })
    }

    /// Antwort auf `navigator.credentials.get()` (`options` ist das Feld `options` des Servers).
    pub fn authenticate(&mut self, options: &Value) -> Value {
        let challenge = options["publicKey"]["challenge"]
            .as_str()
            .expect("challenge");
        self.counter += 1;

        let client_data = json!({
            "type": "webauthn.get",
            "challenge": challenge,
            "origin": self.origin,
            "crossOrigin": false,
        })
        .to_string();

        let mut auth_data = Sha256::digest(self.rp_id.as_bytes()).to_vec();
        auth_data.push(0x01 | 0x04);
        auth_data.extend_from_slice(&self.counter.to_be_bytes());

        let mut signed = auth_data.clone();
        signed.extend_from_slice(&Sha256::digest(client_data.as_bytes()));
        let signature: Signature = self.key.sign(&signed);

        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": {
                "authenticatorData": b64(&auth_data),
                "clientDataJSON": b64(client_data.as_bytes()),
                "signature": b64(signature.to_der().as_bytes()),
                "userHandle": self.user_handle.as_deref().map(b64),
            },
            "clientExtensionResults": {},
        })
    }
}
