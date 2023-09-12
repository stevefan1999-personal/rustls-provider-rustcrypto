use core::marker::PhantomData;

use aead::{AeadInPlace, KeyInit, KeySizeUser};
use alloc::{boxed::Box, vec::Vec};
use rustls::{
    crypto::cipher::{self, MessageEncrypter},
    internal::{cipher::MessageDecrypter, msgs::base::Payload},
    ContentType, ProtocolVersion,
};
type NonceSize = [u8; 12];

trait AeadMetaTls13 {
    const OVERHEAD: usize;
}

trait AeadMetaTls12 {
    const OVERHEAD: usize;

    fn key_block_shape() -> cipher::KeyBlockShape;
}

pub struct Aead<T>(PhantomData<T>);

impl<T> Aead<T> {
    pub const DEFAULT: Self = Self(PhantomData);
}

impl<T> cipher::Tls13AeadAlgorithm for Aead<T>
where
    T: Send + Sync + KeyInit + KeySizeUser + aead::AeadInPlace + 'static,
    aead::Nonce<T>: From<NonceSize>,
    AeadCipherTls13<T>: AeadMetaTls13,
{
    fn encrypter(&self, key: cipher::AeadKey, iv: cipher::Iv) -> Box<dyn cipher::MessageEncrypter> {
        Box::new(AeadCipherTls13(
            T::new_from_slice(key.as_ref()).unwrap(),
            iv,
        ))
    }

    fn decrypter(&self, key: cipher::AeadKey, iv: cipher::Iv) -> Box<dyn cipher::MessageDecrypter> {
        Box::new(AeadCipherTls13(
            T::new_from_slice(key.as_ref()).unwrap(),
            iv,
        ))
    }

    fn key_len(&self) -> usize {
        T::key_size()
    }
}

impl<T> cipher::Tls12AeadAlgorithm for Aead<T>
where
    T: Send + Sync + KeyInit + KeySizeUser + aead::AeadInPlace + 'static,
    aead::Nonce<T>: From<NonceSize>,
    AeadCipherTls12<T>: AeadMetaTls12,
{
    fn encrypter(
        &self,
        key: cipher::AeadKey,
        iv: &[u8],
        _extra: &[u8],
    ) -> Box<dyn MessageEncrypter> {
        Box::new(AeadCipherTls12(
            T::new_from_slice(key.as_ref()).unwrap(),
            cipher::Iv::copy(iv),
        ))
    }

    fn decrypter(&self, key: cipher::AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        Box::new(AeadCipherTls12(
            T::new_from_slice(key.as_ref()).unwrap(),
            cipher::Iv::copy(iv),
        ))
    }

    fn key_block_shape(&self) -> cipher::KeyBlockShape {
        AeadCipherTls12::<T>::key_block_shape()
    }
}

struct AeadCipherTls13<T>(T, cipher::Iv);

impl<T> MessageDecrypter for AeadCipherTls13<T>
where
    T: Send + Sync + AeadInPlace,
    aead::Nonce<T>: From<NonceSize>,
{
    fn decrypt(
        &self,
        mut m: cipher::OpaqueMessage,
        seq: u64,
    ) -> Result<cipher::PlainMessage, rustls::Error> {
        let payload = &mut m.payload.0;
        let nonce = cipher::Nonce::new(&self.1, seq).0;
        let aad = cipher::make_tls13_aad(payload.len());

        self.0
            .decrypt_in_place(&nonce.into(), &aad, payload)
            .map_err(|_| rustls::Error::DecryptError)?;

        m.into_tls13_unpadded_message()
    }
}

impl<T> MessageEncrypter for AeadCipherTls13<T>
where
    T: Send + Sync + AeadInPlace,
    aead::Nonce<T>: From<NonceSize>,
    AeadCipherTls13<T>: AeadMetaTls13,
{
    fn encrypt(
        &self,
        m: cipher::BorrowedPlainMessage,
        seq: u64,
    ) -> Result<cipher::OpaqueMessage, rustls::Error> {
        let total_len = m.payload.len() + 1 + <Self as AeadMetaTls13>::OVERHEAD;

        // construct a TLSInnerPlaintext
        let mut payload = Vec::with_capacity(total_len);
        payload.extend_from_slice(m.payload);
        payload.push(m.typ.get_u8());

        let nonce = cipher::Nonce::new(&self.1, seq).0;
        let aad = cipher::make_tls13_aad(total_len);

        self.0
            .encrypt_in_place(&nonce.into(), &aad, &mut payload)
            .map_err(|_| rustls::Error::EncryptError)
            .and_then(|_| {
                Ok(cipher::OpaqueMessage {
                    typ: ContentType::ApplicationData,
                    version: ProtocolVersion::TLSv1_2,
                    payload: Payload::new(payload),
                })
            })
    }
}

struct AeadCipherTls12<T>(T, cipher::Iv);

impl<T> cipher::MessageEncrypter for AeadCipherTls12<T>
where
    T: Send + Sync + AeadInPlace,
    aead::Nonce<T>: From<NonceSize>,
    AeadCipherTls12<T>: AeadMetaTls12,
{
    fn encrypt(
        &self,
        m: cipher::BorrowedPlainMessage,
        seq: u64,
    ) -> Result<cipher::OpaqueMessage, rustls::Error> {
        let total_len = m.payload.len() + <Self as AeadMetaTls12>::OVERHEAD;

        let mut payload = Vec::with_capacity(total_len);
        payload.extend_from_slice(m.payload);

        let nonce = cipher::Nonce::new(&self.1, seq).0;
        let aad = cipher::make_tls12_aad(seq, m.typ, m.version, payload.len());

        self.0
            .encrypt_in_place(&nonce.into(), &aad, &mut payload)
            .map_err(|_| rustls::Error::EncryptError)
            .and_then(|_| {
                Ok(cipher::OpaqueMessage {
                    typ: m.typ,
                    version: m.version,
                    payload: Payload::new(payload),
                })
            })
    }
}

impl<T> cipher::MessageDecrypter for AeadCipherTls12<T>
where
    T: Send + Sync + AeadInPlace,
    aead::Nonce<T>: From<NonceSize>,
    AeadCipherTls12<T>: AeadMetaTls12,
{
    fn decrypt(
        &self,
        mut m: cipher::OpaqueMessage,
        seq: u64,
    ) -> Result<cipher::PlainMessage, rustls::Error> {
        let payload = &mut m.payload.0;
        let nonce = cipher::Nonce::new(&self.1, seq).0;
        let aad = cipher::make_tls12_aad(
            seq,
            m.typ,
            m.version,
            payload.len() - <Self as AeadMetaTls12>::OVERHEAD,
        );

        self.0
            .decrypt_in_place(&nonce.into(), &aad, payload)
            .map_err(|_| rustls::Error::DecryptError)?;

        Ok(m.into_plain_message())
    }
}

impl AeadMetaTls13 for AeadCipherTls13<chacha20poly1305::ChaCha20Poly1305> {
    const OVERHEAD: usize = 16;
}

impl AeadMetaTls12 for AeadCipherTls12<chacha20poly1305::ChaCha20Poly1305> {
    const OVERHEAD: usize = 16;

    fn key_block_shape() -> cipher::KeyBlockShape {
        cipher::KeyBlockShape {
            enc_key_len: 32,
            fixed_iv_len: 12,
            explicit_nonce_len: 0,
        }
    }
}

impl AeadMetaTls13 for AeadCipherTls13<aes_gcm::Aes128Gcm> {
    const OVERHEAD: usize = 16;
}

impl AeadMetaTls13 for AeadCipherTls13<aes_gcm::Aes256Gcm> {
    const OVERHEAD: usize = 16;
}

// TODO
// impl AeadMetaTls12 for AeadCipherTls12<aes_gcm::Aes256Gcm> {
//     const OVERHEAD: usize = 24;

//     fn key_block_shape() -> cipher::KeyBlockShape {
//         cipher::KeyBlockShape {
//             enc_key_len: aes_gcm::Aes256Gcm::key_size(),
//             fixed_iv_len: 4,
//             explicit_nonce_len: 8,
//         }
//     }
// }