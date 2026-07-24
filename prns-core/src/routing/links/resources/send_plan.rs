use super::{ResourceSegment, MAX_EFFICIENT_SIZE, METADATA_PREFIX_LEN};

#[derive(Debug, PartialEq, Eq)]
pub enum ResourceSendPlanError {
    ZeroSegmentBytes,
    SegmentTooLarge,
    PackedMetadataLengthOverflow,
    TotalLengthOverflow,
    MetadataDoesNotFit,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResourceSendPlan {
    total_data_bytes: u64,
    metadata_block_bytes: u64,
    segment_stream_bytes: u64,
    total_stream_bytes: u64,
    total_segments: u64,
    rebalance_final_pair: bool,
}

impl ResourceSendPlan {
    pub fn new(
        total_data_bytes: u64,
        packed_metadata_bytes: Option<u64>,
        segment_stream_bytes: u64,
    ) -> Result<Self, ResourceSendPlanError> {
        if segment_stream_bytes == 0 {
            return Err(ResourceSendPlanError::ZeroSegmentBytes);
        }
        if segment_stream_bytes > MAX_EFFICIENT_SIZE as u64 {
            return Err(ResourceSendPlanError::SegmentTooLarge);
        }
        let metadata_block_bytes = match packed_metadata_bytes {
            Some(packed_bytes) => packed_bytes
                .checked_add(METADATA_PREFIX_LEN as u64)
                .ok_or(ResourceSendPlanError::PackedMetadataLengthOverflow)?,
            None => 0,
        };
        if metadata_block_bytes > segment_stream_bytes {
            return Err(ResourceSendPlanError::MetadataDoesNotFit);
        }
        let total_stream_bytes = total_data_bytes
            .checked_add(metadata_block_bytes)
            .ok_or(ResourceSendPlanError::TotalLengthOverflow)?;
        let total_segments = total_stream_bytes.div_ceil(segment_stream_bytes).max(1);
        let final_stream_bytes = total_stream_bytes % segment_stream_bytes;
        let rebalance_final_pair = total_segments > 1
            && final_stream_bytes != 0
            && final_stream_bytes < segment_stream_bytes / 2;
        Ok(Self {
            total_data_bytes,
            metadata_block_bytes,
            segment_stream_bytes,
            total_stream_bytes,
            total_segments,
            rebalance_final_pair,
        })
    }

    #[must_use]
    pub const fn total_stream_bytes(&self) -> u64 {
        self.total_stream_bytes
    }

    #[must_use]
    pub const fn total_segments(&self) -> u64 {
        self.total_segments
    }

    #[must_use]
    pub fn segment(&self, index: u64) -> Option<ResourceSegmentPlan> {
        if index == 0 || index > self.total_segments {
            return None;
        }
        let full_segment_count = if self.rebalance_final_pair {
            self.total_segments.saturating_sub(2)
        } else {
            self.total_segments
        };
        let (stream_start, stream_bytes) = if index <= full_segment_count {
            let stream_start = index.saturating_sub(1) * self.segment_stream_bytes;
            (
                stream_start,
                self.total_stream_bytes
                    .saturating_sub(stream_start)
                    .min(self.segment_stream_bytes),
            )
        } else if self.rebalance_final_pair && index == self.total_segments - 1 {
            let stream_start = full_segment_count * self.segment_stream_bytes;
            let stream_remaining = self.total_stream_bytes.saturating_sub(stream_start);
            let first_stream_bytes = stream_remaining
                .div_ceil(2)
                .max(if full_segment_count == 0 {
                    self.metadata_block_bytes
                } else {
                    0
                });
            (stream_start, first_stream_bytes)
        } else {
            let pair_start = full_segment_count * self.segment_stream_bytes;
            let pair_bytes = self.total_stream_bytes.saturating_sub(pair_start);
            let first_stream_bytes = pair_bytes.div_ceil(2).max(if full_segment_count == 0 {
                self.metadata_block_bytes
            } else {
                0
            });
            let stream_start = pair_start.saturating_add(first_stream_bytes);
            (
                stream_start,
                self.total_stream_bytes.saturating_sub(stream_start),
            )
        };
        let data_start = stream_start.saturating_sub(self.metadata_block_bytes);
        let data_end = stream_start
            .saturating_add(stream_bytes)
            .saturating_sub(self.metadata_block_bytes)
            .min(self.total_data_bytes);
        Some(ResourceSegmentPlan {
            segment: ResourceSegment {
                index,
                total_segments: self.total_segments,
                total_data_bytes: self.total_data_bytes,
            },
            data_start,
            data_end,
            stream_bytes,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResourceSegmentPlan {
    pub segment: ResourceSegment,
    pub data_start: u64,
    pub data_end: u64,
    pub stream_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_resource_is_one_empty_segment() -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(0, None, 100)?;
        assert_eq!(plan.total_stream_bytes(), 0);
        assert_eq!(plan.total_segments(), 1);
        assert_eq!(
            plan.segment(1),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment::whole(0),
                data_start: 0,
                data_end: 0,
                stream_bytes: 0,
            })
        );
        Ok(())
    }

    #[test]
    fn exact_boundary_does_not_create_an_empty_tail() -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(200, None, 100)?;
        assert_eq!(plan.total_segments(), 2);
        assert_eq!(
            plan.segment(2),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 2,
                    total_segments: 2,
                    total_data_bytes: 200,
                },
                data_start: 100,
                data_end: 200,
                stream_bytes: 100,
            })
        );
        Ok(())
    }

    #[test]
    fn metadata_occupies_the_front_of_segment_one() -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(150, Some(17), 100)?;
        assert_eq!(plan.total_stream_bytes(), 170);
        assert_eq!(
            plan.segment(1),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 150,
                },
                data_start: 0,
                data_end: 80,
                stream_bytes: 100,
            })
        );
        assert_eq!(
            plan.segment(2),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 2,
                    total_segments: 2,
                    total_data_bytes: 150,
                },
                data_start: 80,
                data_end: 150,
                stream_bytes: 70,
            })
        );
        Ok(())
    }

    #[test]
    fn tiny_tail_is_balanced_across_the_final_pair() -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(210, None, 100)?;
        assert_eq!(plan.total_segments(), 3);
        assert_eq!(
            plan.segment(2),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 2,
                    total_segments: 3,
                    total_data_bytes: 210,
                },
                data_start: 100,
                data_end: 155,
                stream_bytes: 55,
            })
        );
        assert_eq!(
            plan.segment(3),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 3,
                    total_segments: 3,
                    total_data_bytes: 210,
                },
                data_start: 155,
                data_end: 210,
                stream_bytes: 55,
            })
        );
        Ok(())
    }

    #[test]
    fn metadata_block_remains_whole_when_the_final_pair_rebalances(
    ) -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(20, Some(87), 100)?;
        assert_eq!(plan.total_stream_bytes(), 110);
        assert_eq!(plan.total_segments(), 2);
        assert_eq!(
            plan.segment(1),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 20,
                },
                data_start: 0,
                data_end: 0,
                stream_bytes: 90,
            })
        );
        assert_eq!(
            plan.segment(2),
            Some(ResourceSegmentPlan {
                segment: ResourceSegment {
                    index: 2,
                    total_segments: 2,
                    total_data_bytes: 20,
                },
                data_start: 0,
                data_end: 20,
                stream_bytes: 20,
            })
        );
        Ok(())
    }

    #[test]
    fn invalid_indices_have_no_segment() -> Result<(), ResourceSendPlanError> {
        let plan = ResourceSendPlan::new(10, None, 100)?;
        assert_eq!(plan.segment(0), None);
        assert_eq!(plan.segment(2), None);
        Ok(())
    }
}
