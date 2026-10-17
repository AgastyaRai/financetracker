use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Payload},
};
use uuid::Uuid;

const FORMAT_VERSION: u8 = 1;
const NONCE_LENGTH: usize = 24;
const TAG_LENGTH: usize = 16;
const HEADER_LENGTH: usize = 1 + NONCE_LENGTH;

// keep the owner and external connection identity together when binding a credential
#[derive(Clone, Copy)]
pub struct CredentialContext<'a> {
    pub user_id: Uuid,
    pub provider: &'a str,
    pub provider_id: &'a str,
}

impl CredentialContext<'_> {
    // encoding separate fields prevents ambiguous identities when provider IDs contain separators
    fn associated_data(&self) -> Result<Vec<u8>, CredentialError> {
        serde_json::to_vec(&(self.user_id, self.provider, self.provider_id))
            .map_err(|_| CredentialError)
    }
}

// failures expose neither the key nor the credential to a future caller's error response
#[derive(Debug)]
pub struct CredentialError;

// configuration failures retain only the reason, never the supplied key or environment error payload
#[derive(Debug)]
pub enum CredentialKeyError {
    InvalidEnvironment,
    InvalidBase64,
    InvalidLength,
}

impl std::fmt::Display for CredentialKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::InvalidEnvironment => "must contain valid Unicode text",
            Self::InvalidBase64 => "must use standard padded Base64 without whitespace",
            Self::InvalidLength => "must decode to exactly 32 bytes",
        };
        write!(formatter, "BANK_CREDENTIAL_ENCRYPTION_KEY {reason}")
    }
}

impl std::error::Error for CredentialKeyError {}

pub struct BankCredentialCipher {
    cipher: XChaCha20Poly1305,
}

impl BankCredentialCipher {
    // the caller supplies the key; this helper does not choose where keys are stored
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    // keep missing settings distinct from invalid ones without reading process-wide state here
    pub fn from_env_value(
        value: Result<String, std::env::VarError>,
    ) -> Result<Option<Self>, CredentialKeyError> {
        let encoded_key = match value {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(CredentialKeyError::InvalidEnvironment);
            }
        };

        // decode the configured value as-is; trimming or deriving another key could hide a mistake
        let decoded_key = STANDARD
            .decode(encoded_key)
            .map_err(|_| CredentialKeyError::InvalidBase64)?;
        let key: [u8; 32] = decoded_key
            .try_into()
            .map_err(|_| CredentialKeyError::InvalidLength)?;
        Ok(Some(Self::new(&key)))
    }

    pub fn encrypt(
        &self,
        access_token: &str,
        context: &CredentialContext<'_>,
    ) -> Result<Vec<u8>, CredentialError> {
        // each write gets a fresh operating-system nonce; randomness failure aborts the write
        let nonce = XNonce::try_generate().map_err(|_| CredentialError)?;
        let associated_data = context.associated_data()?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: access_token.as_bytes(),
                    aad: &associated_data,
                },
            )
            .map_err(|_| CredentialError)?;

        // the encrypted payload already includes its tag; retain the nonce so another instance can read it
        let mut stored_credential = Vec::with_capacity(HEADER_LENGTH + ciphertext.len());
        stored_credential.push(FORMAT_VERSION);
        stored_credential.extend_from_slice(&nonce);
        stored_credential.extend_from_slice(&ciphertext);
        Ok(stored_credential)
    }

    pub fn decrypt(
        &self,
        stored_credential: &[u8],
        context: &CredentialContext<'_>,
    ) -> Result<String, CredentialError> {
        // validate the minimum layout before indexing or slicing possibly damaged database bytes
        if stored_credential.len() < HEADER_LENGTH + TAG_LENGTH
            || stored_credential[0] != FORMAT_VERSION
        {
            return Err(CredentialError);
        }
        let nonce_bytes: [u8; NONCE_LENGTH] = stored_credential[1..HEADER_LENGTH]
            .try_into()
            .map_err(|_| CredentialError)?;
        let nonce = XNonce::from(nonce_bytes);
        let associated_data = context.associated_data()?;

        // authentication must succeed for both the stored bytes and the expected owner/connection
        let plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &stored_credential[HEADER_LENGTH..],
                    aad: &associated_data,
                },
            )
            .map_err(|_| CredentialError)?;
        String::from_utf8(plaintext).map_err(|_| CredentialError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use base64::{Engine, engine::general_purpose::STANDARD};

    const TEST_KEY: [u8; 32] = [42; 32];
    const TEST_TOKEN: &str = "access-sandbox-encryption-test-token";

    fn test_context() -> CredentialContext<'static> {
        CredentialContext {
            user_id: Uuid::from_u128(1),
            provider: "plaid",
            provider_id: "test-provider-connection",
        }
    }

    // repeated writes need fresh nonces, and a new instance with the same key must still read them
    #[test]
    fn test_repeated_encryption_uses_fresh_nonces_and_decrypts_with_same_key() {
        // use a fixed test key without reading configuration or generating a persistent key
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = test_context();
        let first = cipher.encrypt(TEST_TOKEN, &context).unwrap();
        let second = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        // the storage contract is one version byte, a 24-byte nonce, ciphertext, and a 16-byte tag
        assert_eq!(first[0], 1);
        assert_eq!(second[0], 1);
        assert_eq!(first.len(), 1 + 24 + TEST_TOKEN.len() + 16);
        assert_eq!(second.len(), first.len());
        assert_ne!(&first[1..25], &second[1..25]);
        assert_ne!(&first[25..], &second[25..]);

        // recovery depends on the supplied key and stored bytes, not hidden state in the writer
        let reader = BankCredentialCipher::new(&TEST_KEY);
        assert_eq!(reader.decrypt(&first, &context).unwrap(), TEST_TOKEN);
        assert_eq!(reader.decrypt(&second, &context).unwrap(), TEST_TOKEN);
    }

    // using another key must fail authentication rather than return a plausible token
    #[test]
    fn test_decryption_rejects_wrong_key() {
        let context = test_context();
        let stored = BankCredentialCipher::new(&TEST_KEY)
            .encrypt(TEST_TOKEN, &context)
            .unwrap();

        let wrong_key = [43; 32];
        let reader = BankCredentialCipher::new(&wrong_key);
        assert!(reader.decrypt(&stored, &context).is_err());
    }

    // copying a credential to another owner or external connection must not make it usable there
    #[test]
    fn test_decryption_rejects_wrong_owner_or_connection() {
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = test_context();
        let stored = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        // change each part separately so no identity field can silently be left out of the binding
        let other_contexts = [
            CredentialContext {
                user_id: Uuid::from_u128(2),
                ..context
            },
            CredentialContext {
                provider: "other-provider",
                ..context
            },
            CredentialContext {
                provider_id: "other-connection",
                ..context
            },
        ];
        for other_context in other_contexts {
            assert!(cipher.decrypt(&stored, &other_context).is_err());
        }
        assert_eq!(cipher.decrypt(&stored, &context).unwrap(), TEST_TOKEN);
    }

    // identity fields containing separators must remain distinct instead of sharing a joined string
    #[test]
    fn test_connection_binding_keeps_identity_fields_separate() {
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = CredentialContext {
            provider: "plaid:region",
            provider_id: "connection",
            ..test_context()
        };
        let stored = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        // these identities would collide if provider and provider_id were simply joined with ':'
        let other_context = CredentialContext {
            provider: "plaid",
            provider_id: "region:connection",
            ..context
        };
        assert!(cipher.decrypt(&stored, &other_context).is_err());
    }

    // changing the nonce, encrypted token, or authentication tag must all invalidate the stored value
    #[test]
    fn test_decryption_rejects_tampered_credential() {
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = test_context();
        let stored = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        // alter one region at a time so a nonce-only change is checked as well as ciphertext damage
        for (position, region) in [(1, "nonce"), (25, "ciphertext"), (stored.len() - 1, "tag")] {
            let mut tampered = stored.clone();
            tampered[position] ^= 1;
            assert!(
                cipher.decrypt(&tampered, &context).is_err(),
                "accepted modified {region}"
            );
        }
    }

    // incomplete database bytes should return an error at every truncation point without panicking
    #[test]
    fn test_decryption_rejects_truncated_credential() {
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = test_context();
        let stored = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        // include empty input, partial headers, and partially stored ciphertext/tag
        for length in 0..stored.len() {
            assert!(
                cipher.decrypt(&stored[..length], &context).is_err(),
                "accepted {length}-byte prefix"
            );
        }
    }

    // an unknown format must not be interpreted as this version's nonce and ciphertext layout
    #[test]
    fn test_decryption_rejects_unsupported_format_version() {
        let cipher = BankCredentialCipher::new(&TEST_KEY);
        let context = test_context();
        let mut stored = cipher.encrypt(TEST_TOKEN, &context).unwrap();

        stored[0] = 2;
        assert!(cipher.decrypt(&stored, &context).is_err());
    }

    // decoding must preserve the actual configured bytes rather than substitute a different key
    #[test]
    fn test_key_loader_uses_decoded_key_bytes() {
        // this fixed fixture encodes the bytes 0 through 31, independently of the loader
        let encoded_key = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
        let expected_key = std::array::from_fn(|index| index as u8);
        let context = test_context();
        let stored = BankCredentialCipher::new(&expected_key)
            .encrypt(TEST_TOKEN, &context)
            .unwrap();

        // a cipher loaded from the environment value must read credentials written with those bytes
        let loaded = BankCredentialCipher::from_env_value(Ok(encoded_key.to_string()))
            .unwrap()
            .expect("configured key should produce a cipher");
        assert_eq!(loaded.decrypt(&stored, &context).unwrap(), TEST_TOKEN);
    }

    // reloading a persistent setting must not generate a replacement key and strand saved credentials
    #[test]
    fn test_key_loader_reloads_same_key_for_existing_credentials() {
        let encoded_key = STANDARD.encode(TEST_KEY);
        let context = test_context();
        let writer = BankCredentialCipher::from_env_value(Ok(encoded_key.clone()))
            .unwrap()
            .expect("configured key should produce a cipher");
        let stored = writer.encrypt(TEST_TOKEN, &context).unwrap();

        // simulate a later process startup using only the same configuration and stored bytes
        let reader = BankCredentialCipher::from_env_value(Ok(encoded_key))
            .unwrap()
            .expect("reloading the key should produce a cipher");
        assert_eq!(reader.decrypt(&stored, &context).unwrap(), TEST_TOKEN);
    }

    // an absent optional key must leave existing application routes usable without inventing a key
    #[test]
    fn test_key_loader_leaves_missing_key_unconfigured() {
        let loaded = BankCredentialCipher::from_env_value(Err(std::env::VarError::NotPresent))
            .expect("a missing optional key should not prevent startup");
        assert!(loaded.is_none());
    }

    // a supplied but empty or whitespace-damaged setting is a configuration error, not an absent key
    #[test]
    fn test_key_loader_rejects_empty_or_whitespace_values() {
        let encoded_key = STANDARD.encode(TEST_KEY);
        let values = [
            String::new(),
            " ".to_string(),
            "\t\r\n".to_string(),
            format!(" {encoded_key}"),
            format!("{encoded_key} "),
            format!("{encoded_key}\n"),
        ];

        // reject surrounding whitespace instead of silently correcting the configured secret
        for value in values {
            assert!(BankCredentialCipher::from_env_value(Ok(value)).is_err());
        }
    }

    // accept one documented encoding so malformed values and other Base64 formats fail explicitly
    #[test]
    fn test_key_loader_rejects_malformed_base64() {
        let encoded_key = STANDARD.encode(TEST_KEY);
        let values = [
            "not-a-valid-base64-key!".to_string(),
            encoded_key.trim_end_matches('=').to_string(),
            STANDARD.encode([255; 32]).replace('/', "_"),
        ];

        for value in values {
            assert!(matches!(
                BankCredentialCipher::from_env_value(Ok(value)),
                Err(CredentialKeyError::InvalidBase64)
            ));
        }
    }

    // valid text encoding is insufficient when the decoded key does not match the cipher's 32-byte size
    #[test]
    fn test_key_loader_rejects_wrong_decoded_length() {
        for length in [31, 33] {
            let encoded_key = STANDARD.encode(vec![42; length]);
            assert!(matches!(
                BankCredentialCipher::from_env_value(Ok(encoded_key)),
                Err(CredentialKeyError::InvalidLength)
            ));
        }
    }

    // an unreadable environment value must not be silently treated like a missing optional key
    #[test]
    fn test_key_loader_rejects_invalid_environment_text() {
        // inject the lookup failure directly so this case works on Windows without mutating the environment
        let lookup_error =
            std::env::VarError::NotUnicode(std::ffi::OsString::from("test-key-marker"));
        assert!(matches!(
            BankCredentialCipher::from_env_value(Err(lookup_error)),
            Err(CredentialKeyError::InvalidEnvironment)
        ));
    }

    // startup diagnostics should identify the setting and problem without retaining secret-bearing inputs
    #[test]
    fn test_key_loader_errors_do_not_expose_supplied_values() {
        let malformed_key = "sensitive-invalid-key!".to_string();
        let short_key = STANDARD.encode([19; 31]);
        let unreadable_key = "sensitive-environment-marker".to_string();
        let cases = [
            (Ok(malformed_key.clone()), malformed_key, "Base64"),
            (Ok(short_key.clone()), short_key, "32 bytes"),
            (
                Err(std::env::VarError::NotUnicode(std::ffi::OsString::from(
                    &unreadable_key,
                ))),
                unreadable_key,
                "Unicode",
            ),
        ];

        // check Display for startup messages and Debug for accidental diagnostic formatting
        for (lookup_result, supplied_value, reason) in cases {
            let error = BankCredentialCipher::from_env_value(lookup_result)
                .err()
                .expect("invalid configuration should return a diagnostic");
            let message = error.to_string();
            assert!(message.contains("BANK_CREDENTIAL_ENCRYPTION_KEY"));
            assert!(message.contains(reason));
            assert!(!message.contains(&supplied_value));
            assert!(!format!("{error:?}").contains(&supplied_value));
        }
    }
}
