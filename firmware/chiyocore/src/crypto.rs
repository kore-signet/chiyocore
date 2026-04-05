use esp_hal::aes::{AesContext, cipher_modes::Ecb};
use hmac::SimpleHmac;
use meshcore::{
    crypto::{AesImpl, HmacImpl},
    io::ByteVecImpl,
};
use sha2::digest::{FixedOutput, KeyInit, Update, generic_array::GenericArray};

pub const CIPHER_BLOCK_SIZE: usize = 16;

/// Hardware accelerated AES impl, using esp-hal.
pub struct HardwareAES;

impl AesImpl for HardwareAES {
    type Error = esp_hal::aes::Error;

    async fn decrypt<'s>(
        key: &[u8; 16],
        input: &[u8],
        output: &'s mut impl ByteVecImpl,
    ) -> Result<&'s [u8], Self::Error> {
        let mut context = AesContext::new(Ecb, esp_hal::aes::Operation::Decrypt, *key);
        output.resize(input.len(), 0);
        context.process(input, output)?.wait().await;
        Ok(&output[..])
    }

    async fn encrypt<'s>(
        key: &[u8; 16],
        input: &[u8],
        output: &'s mut impl ByteVecImpl,
    ) -> Result<&'s [u8], Self::Error> {
        let mut context = AesContext::new(Ecb, esp_hal::aes::Operation::Encrypt, *key);
        let pad_len = (CIPHER_BLOCK_SIZE - (input.len() % CIPHER_BLOCK_SIZE)) % CIPHER_BLOCK_SIZE;
        output.resize(input.len() + pad_len, 0);

        context.process(input, output)?.wait().await;

        Ok(&output[..])
    }

    async fn decrypt_in_place<'s>(
        key: &[u8; 16],
        data: &'s mut impl ByteVecImpl,
    ) -> Result<&'s [u8], Self::Error> {
        let mut context = AesContext::new(Ecb, esp_hal::aes::Operation::Decrypt, *key);
        context.process_in_place(data)?.wait().await;
        Ok(&data[..])
    }

    async fn encrypt_in_place<'s>(
        key: &[u8; 16],
        data: &'s mut impl ByteVecImpl,
    ) -> Result<&'s [u8], Self::Error> {
        let pad_len = (CIPHER_BLOCK_SIZE - (data.len() % CIPHER_BLOCK_SIZE)) % CIPHER_BLOCK_SIZE;
        data.resize(data.len() + pad_len, 0);

        let mut context = AesContext::new(Ecb, esp_hal::aes::Operation::Encrypt, *key);

        context.process_in_place(data)?.wait().await;
        Ok(&data[..])
    }
}

pub type HardwareSHA = esp_hal::sha::Sha256Context;

/// Hardware accelerated HMAC impl, using esp-hal's SHA accelerator.
pub struct HardwareHMAC;

impl HmacImpl for HardwareHMAC {
    fn mac(val: &[u8], mac_key: &[u8]) -> [u8; 32] {
        let mut hyb_hmac =
            SimpleHmac::<esp_hal::sha::Sha256Context>::new_from_slice(mac_key).unwrap();
        hyb_hmac.update(val);
        let mut out = [0u8; 32];
        hyb_hmac.finalize_into(GenericArray::from_mut_slice(&mut out));
        out
    }
}
