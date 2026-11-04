use super::*;

impl<B: AtomicBackend + Send> Adapter<B> {
    pub(super) fn prepare_remove_member(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        binding: &crate::MemberBinding,
    ) -> Result<Output, Error<B::Error>> {
        self.owner()?;
        let state = self.load(provider, group)?;
        let member = state
            .roster()
            .members_iter()
            .find(|member| binding.matches(&member.signing_identity))
            .ok_or(PolicyError::Credential)?;
        self.prepare_remove(provider, group, member.index)
    }

    pub(super) fn prepare_retire(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<Output, Error<B::Error>> {
        let state = self.load(provider, group)?;
        let epoch = state.current_epoch();
        provider.purge_epochs(&group.0)?;
        provider.delete(&key(b'G', &group.0))?;
        provider.delete(&key(b'P', &group.0))?;
        provider.put(&key(b'R', &group.0), &[2])?;
        provider.put(&key(b'B', &group.0), &epoch.to_be_bytes())?;
        Ok(Self::context(Output::empty(Event::Retired), group, epoch))
    }
}
