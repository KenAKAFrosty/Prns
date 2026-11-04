use alloc::{sync::Arc, vec, vec::Vec};
use mls_rs::{
    group::{
        mls_rules::{CommitDirection, CommitOptions, CommitSource, EncryptionOptions, MlsRules},
        GroupContext, Roster,
    },
    identity::{basic::BasicCredential, SigningIdentity},
    mls_rules::ProposalBundle,
    CipherSuite, CipherSuiteProvider, CryptoProvider,
};
use mls_rs_core::{
    crypto::SignatureSecretKey,
    error::IntoAnyError,
    extension::ExtensionList,
    identity::{CredentialType, IdentityProvider, MemberValidationContext},
    time::MlsTime,
};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberBinding {
    pub endpoint: [u8; 32],
    pub incarnation: u64,
    pub signing_key: [u8; 32],
}
impl MemberBinding {
    fn credential(&self) -> Vec<u8> {
        let mut bytes = self.endpoint.to_vec();
        bytes.extend_from_slice(&self.incarnation.to_be_bytes());
        bytes
    }
    pub(crate) fn matches(&self, identity: &SigningIdentity) -> bool {
        identity
            .credential
            .as_basic()
            .is_some_and(|basic| basic.identifier == self.credential())
            && identity.signature_key.as_ref() == self.signing_key
    }
    /// Verify a domain-separated enrollment manifest signature with this pinned key.
    pub fn verify_enrollment(&self, message: &[u8], signature: &[u8]) -> Result<(), PolicyError> {
        let provider =
            RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_AES128]);
        let suite = provider
            .cipher_suite_provider(CipherSuite::CURVE25519_AES128)
            .ok_or(PolicyError::Crypto)?;
        let mut signed = b"prns group enrollment manifest v1\0".to_vec();
        signed.extend_from_slice(message);
        suite
            .verify(&self.signing_key.to_vec().into(), signature, &signed)
            .map_err(|_| PolicyError::Credential)
    }
}

/// Provision this identity durably before advertising it. The adapter never
/// generates a replacement identity while opening an existing store.
#[derive(Clone)]
pub struct LocalIdentity {
    pub(crate) binding: MemberBinding,
    pub(crate) secret: SignatureSecretKey,
}
impl LocalIdentity {
    pub fn generate(endpoint: [u8; 32], incarnation: u64) -> Result<Self, PolicyError> {
        let provider =
            RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_AES128]);
        let suite = provider
            .cipher_suite_provider(CipherSuite::CURVE25519_AES128)
            .ok_or(PolicyError::Crypto)?;
        let (secret, public) = suite
            .signature_key_generate()
            .map_err(|_| PolicyError::Crypto)?;
        let signing_key = public
            .as_ref()
            .try_into()
            .map_err(|_| PolicyError::Crypto)?;
        Ok(Self {
            binding: MemberBinding {
                endpoint,
                incarnation,
                signing_key,
            },
            secret,
        })
    }
    /// Extract zeroizing signing-key material for the platform's protected identity
    /// store. Persist it before publishing any key package or group output.
    pub fn into_parts(self) -> (MemberBinding, SignatureSecretKey) {
        (self.binding, self.secret)
    }
    pub fn binding(&self) -> MemberBinding {
        self.binding.clone()
    }
    pub fn from_parts(
        binding: MemberBinding,
        secret: SignatureSecretKey,
    ) -> Result<Self, PolicyError> {
        let provider =
            RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_AES128]);
        let suite = provider
            .cipher_suite_provider(CipherSuite::CURVE25519_AES128)
            .ok_or(PolicyError::Crypto)?;
        let challenge = b"prns groups local identity validation v1";
        let signature = suite
            .sign(&secret, challenge)
            .map_err(|_| PolicyError::Crypto)?;
        suite
            .verify(&binding.signing_key.to_vec().into(), &signature, challenge)
            .map_err(|_| PolicyError::Credential)?;
        Ok(Self { binding, secret })
    }
    /// Sign public enrollment metadata; private key material remains in this identity.
    pub fn sign_enrollment(&self, message: &[u8]) -> Result<Vec<u8>, PolicyError> {
        let provider =
            RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_AES128]);
        let suite = provider
            .cipher_suite_provider(CipherSuite::CURVE25519_AES128)
            .ok_or(PolicyError::Crypto)?;
        let mut signed = b"prns group enrollment manifest v1\0".to_vec();
        signed.extend_from_slice(message);
        suite
            .sign(&self.secret, &signed)
            .map_err(|_| PolicyError::Crypto)
    }
    pub(crate) fn signing_identity(&self) -> SigningIdentity {
        SigningIdentity::new(
            BasicCredential::new(self.binding.credential()).into_credential(),
            self.binding.signing_key.to_vec().into(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    Credential,
    Owner,
    Members,
    Proposals,
    UnsupportedControl,
    Crypto,
}
impl IntoAnyError for PolicyError {}

/// An explicitly authenticated trust set, not a membership discovery mechanism.
/// `new` fixes all approved bindings. `owner_managed` fixes the owner while its
/// caller supplies an authenticated evolving roster; actual membership is MLS state.
#[derive(Clone)]
pub struct Policy {
    pub(crate) owner: MemberBinding,
    pub(crate) authorized: Arc<Vec<MemberBinding>>,
    owner_managed: bool,
}
impl Policy {
    pub fn new(
        owner: MemberBinding,
        mut authorized: Vec<MemberBinding>,
    ) -> Result<Self, PolicyError> {
        authorized.sort_by_key(|member| (member.endpoint, member.incarnation));
        if !authorized.contains(&owner)
            || authorized
                .windows(2)
                .any(|p| p[0].endpoint == p[1].endpoint)
        {
            return Err(PolicyError::Credential);
        }
        Ok(Self {
            owner,
            authorized: Arc::new(authorized),
            owner_managed: false,
        })
    }
    /// An immutable owner trust root with an authenticated, evolving allowlist.
    /// The caller must validate owner-signed control metadata before changing the
    /// allowlist, and use `compose_with_roster` to bind it to the resulting MLS tree.
    /// This is a separate profile; legacy `new` configuration bytes are unchanged.
    pub fn owner_managed(
        owner: MemberBinding,
        authorized: Vec<MemberBinding>,
    ) -> Result<Self, PolicyError> {
        let mut policy = Self::new(owner, authorized)?;
        policy.owner_managed = true;
        Ok(policy)
    }
    pub fn is_owner_managed(&self) -> bool {
        self.owner_managed
    }
    pub(crate) fn validate(&self, identity: &SigningIdentity) -> Result<(), PolicyError> {
        if self
            .authorized
            .iter()
            .any(|binding| binding.matches(identity))
        {
            Ok(())
        } else {
            Err(PolicyError::Credential)
        }
    }
    pub(crate) fn encode(&self, bytes: &mut Vec<u8>) {
        if self.owner_managed {
            bytes.extend_from_slice(b"prns owner managed policy v1\0");
            bytes.extend_from_slice(&self.owner.credential());
            bytes.extend_from_slice(&self.owner.signing_key);
            return;
        }
        bytes.extend_from_slice(&self.owner.credential());
        bytes.extend_from_slice(&self.owner.signing_key);
        for member in self.authorized.iter() {
            bytes.extend_from_slice(&member.credential());
            bytes.extend_from_slice(&member.signing_key);
        }
    }
}
impl IdentityProvider for Policy {
    type Error = PolicyError;
    fn validate_member(
        &self,
        identity: &SigningIdentity,
        _: Option<MlsTime>,
        _: MemberValidationContext<'_>,
    ) -> Result<(), Self::Error> {
        self.validate(identity)
    }
    fn validate_external_sender(
        &self,
        _: &SigningIdentity,
        _: Option<MlsTime>,
        _: Option<&ExtensionList>,
    ) -> Result<(), Self::Error> {
        Err(PolicyError::UnsupportedControl)
    }
    fn identity(
        &self,
        identity: &SigningIdentity,
        _: &ExtensionList,
    ) -> Result<Vec<u8>, Self::Error> {
        self.validate(identity)?;
        identity
            .credential
            .as_basic()
            .map(|b| b.identifier.clone())
            .ok_or(PolicyError::Credential)
    }
    fn valid_successor(
        &self,
        predecessor: &SigningIdentity,
        successor: &SigningIdentity,
        _: &ExtensionList,
    ) -> Result<bool, Self::Error> {
        self.validate(predecessor)?;
        self.validate(successor)?;
        Ok(predecessor == successor)
    }
    fn supported_types(&self) -> Vec<CredentialType> {
        vec![BasicCredential::credential_type()]
    }
}

#[derive(Clone)]
pub(crate) struct Rules {
    pub policy: Policy,
    pub max_members: usize,
    pub max_proposals: usize,
}
impl MlsRules for Rules {
    type Error = PolicyError;
    fn filter_proposals(
        &self,
        _: CommitDirection,
        source: CommitSource,
        roster: &Roster,
        _: &GroupContext,
        proposals: ProposalBundle,
    ) -> Result<ProposalBundle, Self::Error> {
        let CommitSource::ExistingMember(member) = source else {
            return Err(PolicyError::Owner);
        };
        if !self.policy.owner.matches(&member.signing_identity) {
            return Err(PolicyError::Owner);
        }
        if self.policy.is_owner_managed() {
            if proposals.length() != 1
                || proposals.add_proposals().len() + proposals.remove_proposals().len() != 1
            {
                return Err(PolicyError::UnsupportedControl);
            }
            for proposal in proposals.remove_proposals() {
                let member = roster
                    .member_with_index(proposal.proposal.to_remove())
                    .map_err(|_| PolicyError::Credential)?;
                if self.policy.owner.matches(&member.signing_identity) {
                    return Err(PolicyError::Owner);
                }
            }
        }
        if proposals.length() > self.max_proposals {
            return Err(PolicyError::Proposals);
        }
        if !proposals.reinit_proposals().is_empty()
            || !proposals.external_init_proposals().is_empty()
            || !proposals.group_context_ext_proposals().is_empty()
        {
            return Err(PolicyError::UnsupportedControl);
        }
        Ok(proposals)
    }
    fn commit_options(
        &self,
        roster: &Roster,
        _: &GroupContext,
        _: &ProposalBundle,
    ) -> Result<CommitOptions, Self::Error> {
        if roster.members_iter().count() > self.max_members {
            return Err(PolicyError::Members);
        }
        Ok(CommitOptions::new()
            .with_path_required(true)
            .with_allow_external_commit(false))
    }
    fn encryption_options(
        &self,
        _: &Roster,
        _: &GroupContext,
    ) -> Result<EncryptionOptions, Self::Error> {
        Ok(EncryptionOptions::new(
            true,
            mls_rs::client_builder::PaddingMode::None,
        ))
    }
}

impl core::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MLS policy error: {self:?}")
    }
}
impl core::error::Error for PolicyError {}
