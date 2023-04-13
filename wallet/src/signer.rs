use bitnames_types::*;
use ed25519_dalek_bip32::{ed25519_dalek::Signer as _, *};
use heed::types::*;
use heed::{Database, RoTxn, RwTxn};
use sdk_authorization_ed25519_dalek::get_address;

pub struct Signer {
    seed: [u8; 64],
    address_to_index: Database<SerdeBincode<sdk_types::Address>, OwnedType<u32>>,
    index_to_address: Database<OwnedType<u32>, SerdeBincode<sdk_types::Address>>,
}

impl Signer {
    pub const NUM_DBS: u32 = 2;

    pub fn get_addresses(&self, txn: &RoTxn) -> Result<Vec<Address>, Error> {
        let mut addresses = vec![];
        for item in self.address_to_index.iter(txn)? {
            let (address, _) = &item?;
            addresses.push(*address);
        }
        Ok(addresses)
    }

    pub fn get_new_address(&self, txn: &mut RwTxn) -> Result<Address, Error> {
        let index = self
            .index_to_address
            .last(txn)?
            .map(|(index, _)| index + 1)
            .unwrap_or(0);
        let keypair = self.get_keypair(index)?;
        let address: Address = get_address(&keypair.public);

        self.address_to_index.put(txn, &address, &index)?;
        self.index_to_address.put(txn, &index, &address)?;
        Ok(address)
    }

    pub fn salt(&self, txn: &RoTxn, address: &Address, key: &Key) -> Result<Salt, Error> {
        let index = self
            .address_to_index
            .get(txn, address)?
            .ok_or(Error::AddressDoesNotExist { address: *address })?;
        let keypair = self.get_keypair(index)?;
        let key: &[u8; 32] = key.into();
        let salt: [u8; 32] = blake3::keyed_hash(keypair.secret.as_bytes(), key).into();
        let salt: Salt = salt.into();
        Ok(salt)
    }

    pub fn sign(
        &self,
        txn: &RoTxn,
        address: &Address,
        message: &[u8],
    ) -> Result<(ed25519_dalek::PublicKey, ed25519_dalek::Signature), Error> {
        let index = self
            .address_to_index
            .get(txn, address)?
            .ok_or(Error::AddressDoesNotExist { address: *address })?;
        let keypair = self.get_keypair(index)?;
        Ok((keypair.public, keypair.sign(message)))
    }

    pub fn new(seed: [u8; 64], env: &heed::Env) -> Result<Self, Error> {
        let address_to_index = env.create_database(Some("address_to_index"))?;
        let index_to_address = env.create_database(Some("index_to_address"))?;

        Ok(Self {
            seed,
            address_to_index,
            index_to_address,
        })
    }

    fn get_keypair(&self, index: u32) -> Result<ed25519_dalek::Keypair, Error> {
        let xpriv = ExtendedSecretKey::from_seed(&self.seed)?;
        let derivation_path = DerivationPath::new([
            ChildIndex::Hardened(1),
            ChildIndex::Hardened(0),
            ChildIndex::Hardened(0),
            ChildIndex::Hardened(index),
        ]);
        let child = xpriv.derive(&derivation_path)?;
        let public = child.public_key();
        let secret = child.secret_key;
        Ok(ed25519_dalek::Keypair { secret, public })
    }

    pub fn store() -> Result<(), Error> {
        todo!();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("heed error")]
    Heed(#[from] heed::Error),
    #[error("bip32 error")]
    Bip32(#[from] ed25519_dalek_bip32::Error),
    #[error("address {address} does not exist")]
    AddressDoesNotExist { address: Address },
}
