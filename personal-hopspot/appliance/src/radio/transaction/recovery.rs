use super::*;

const RESTORATION_WINDOW_SECONDS: u64 = 360;
const RECOVERY_CHECK_SECONDS: u64 = 2;
const REBOOT_WINDOW_SECONDS: u64 = 60;

impl<O: ObserveRadioWrites> RadioTransaction<O> {
    pub fn recover(
        &mut self,
        platform: &mut impl RadioPlatform,
    ) -> Result<RadioRecoveryProgress, RadioTransactionError> {
        self.admit()?;
        let (candidate, phase) = match &self.status.state {
            RadioTransactionState::Empty => return Ok(RadioRecoveryProgress::Inactive),
            RadioTransactionState::Active { candidate, phase } => {
                (candidate.clone(), phase.clone())
            }
        };
        let now = platform.time()?;
        match phase {
            RadioPhase::Confirmed | RadioPhase::CanceledBeforeApply | RadioPhase::Restored => {
                Ok(RadioRecoveryProgress::Inactive)
            }
            RadioPhase::Prepared { lease } => {
                if check_lease(&lease, &now).is_err() {
                    self.set_phase(&candidate, RadioPhase::CanceledBeforeApply)?;
                    return Ok(RadioRecoveryProgress::Inactive);
                }
                Ok(wait(&lease, &now))
            }
            RadioPhase::Trial { mut lease } => {
                if lease.boot != now.boot {
                    if lease.trial_boots_remaining == 0 {
                        self.set_phase(&candidate, RadioPhase::Restoring)?;
                        return self.restore(platform, &candidate, now.boot);
                    }
                    lease.trial_boots_remaining -= 1;
                    lease.boot = now.boot.clone();
                    lease.expires_at_uptime_seconds = now
                        .uptime_seconds
                        .checked_add(u64::from(self.guard()?.window.seconds()))
                        .ok_or(RadioTransactionError::LeaseExpired)?;
                    self.set_phase(
                        &candidate,
                        RadioPhase::Trial {
                            lease: lease.clone(),
                        },
                    )?;
                }
                if now.uptime_seconds >= lease.expires_at_uptime_seconds {
                    self.set_phase(&candidate, RadioPhase::Restoring)?;
                    return self.restore(platform, &candidate, now.boot);
                }
                Ok(wait(&lease, &now))
            }
            RadioPhase::Applying { .. } | RadioPhase::Restoring => {
                self.set_phase(&candidate, RadioPhase::Restoring)?;
                self.restore(platform, &candidate, now.boot)
            }
            RadioPhase::RebootRequired {
                requested_from,
                expires_at_uptime_seconds,
            } => {
                if requested_from == now.boot {
                    if now.uptime_seconds >= expires_at_uptime_seconds {
                        self.set_phase(
                            &candidate,
                            RadioPhase::RebootUnavailable { requested_from },
                        )?;
                        return Ok(RadioRecoveryProgress::RebootUnavailable);
                    }
                    platform.request_reboot()?;
                    return Ok(RadioRecoveryProgress::RebootRequested);
                }
                self.begin_restoration_check(platform, &candidate, &now)
            }
            RadioPhase::RebootUnavailable { requested_from } => {
                if requested_from == now.boot {
                    return Ok(RadioRecoveryProgress::RebootUnavailable);
                }
                self.begin_restoration_check(platform, &candidate, &now)
            }
            RadioPhase::CheckingRestoration { lease } => {
                // Another boot cannot renew the post-restore deadline indefinitely.
                if lease.boot != now.boot {
                    self.set_phase(&candidate, RadioPhase::RestorationUnavailable)?;
                    return Ok(RadioRecoveryProgress::RestorationUnavailable);
                }
                self.check_restoration(platform, &candidate, &lease, &now)
            }
            RadioPhase::RestorationUnavailable => {
                self.verify_restored_files(platform)?;
                match platform.restored_readiness(&self.guard()?.profile.binding.device)? {
                    RestoredRadioReadiness::Pending => {
                        Ok(RadioRecoveryProgress::RestorationUnavailable)
                    }
                    RestoredRadioReadiness::Operational => {
                        self.set_phase(&candidate, RadioPhase::Restored)?;
                        Ok(RadioRecoveryProgress::Restored)
                    }
                }
            }
        }
    }

    fn begin_restoration_check(
        &mut self,
        platform: &mut impl RadioPlatform,
        candidate: &RadioCandidateId,
        now: &BootTime,
    ) -> Result<RadioRecoveryProgress, RadioTransactionError> {
        let lease = RadioLease {
            boot: now.boot.clone(),
            expires_at_uptime_seconds: now
                .uptime_seconds
                .checked_add(RESTORATION_WINDOW_SECONDS)
                .ok_or(RadioTransactionError::LeaseExpired)?,
            trial_boots_remaining: 0,
        };
        self.set_phase(
            candidate,
            RadioPhase::CheckingRestoration {
                lease: lease.clone(),
            },
        )?;
        self.check_restoration(platform, candidate, &lease, now)
    }

    fn restore(
        &mut self,
        platform: &mut impl RadioPlatform,
        candidate: &RadioCandidateId,
        boot: BootId,
    ) -> Result<RadioRecoveryProgress, RadioTransactionError> {
        platform.validate(&self.guard()?.plan()?)?;
        let images = RadioFile::ALL
            .iter()
            .map(|file| self.image(file, ImageSource::Original))
            .collect::<Result<Vec<_>, _>>()?;
        self.writes = WriteAdmission::ReopenRequired;
        for (file, image) in RadioFile::ALL.iter().zip(images) {
            platform.replace_synced(file, &image)?;
            self.observer
                .reached(RadioCheckpoint::VendorFileReplaced(file.clone()))?;
        }
        self.set_phase(
            candidate,
            RadioPhase::RebootRequired {
                requested_from: boot,
                expires_at_uptime_seconds: platform
                    .time()?
                    .uptime_seconds
                    .checked_add(REBOOT_WINDOW_SECONDS)
                    .ok_or(RadioTransactionError::LeaseExpired)?,
            },
        )?;
        platform.request_reboot()?;
        Ok(RadioRecoveryProgress::RebootRequested)
    }

    fn verify_restored_files(
        &self,
        platform: &mut impl RadioPlatform,
    ) -> Result<(), RadioTransactionError> {
        for file in RadioFile::ALL {
            if platform.read(&file)? != self.image(&file, ImageSource::Original)? {
                return Err(RadioTransactionError::ConfigurationChanged);
            }
        }
        Ok(())
    }

    fn check_restoration(
        &mut self,
        platform: &mut impl RadioPlatform,
        candidate: &RadioCandidateId,
        lease: &RadioLease,
        now: &BootTime,
    ) -> Result<RadioRecoveryProgress, RadioTransactionError> {
        self.verify_restored_files(platform)?;
        if platform.restored_readiness(&self.guard()?.profile.binding.device)?
            == RestoredRadioReadiness::Operational
        {
            self.set_phase(candidate, RadioPhase::Restored)?;
            return Ok(RadioRecoveryProgress::Restored);
        }
        if now.uptime_seconds >= lease.expires_at_uptime_seconds {
            self.set_phase(candidate, RadioPhase::RestorationUnavailable)?;
            return Ok(RadioRecoveryProgress::RestorationUnavailable);
        }
        Ok(wait(lease, now))
    }
}

fn wait(lease: &RadioLease, now: &BootTime) -> RadioRecoveryProgress {
    RadioRecoveryProgress::Wait {
        seconds: RECOVERY_CHECK_SECONDS
            .min(
                lease
                    .expires_at_uptime_seconds
                    .saturating_sub(now.uptime_seconds),
            )
            .max(1),
    }
}
