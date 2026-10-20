//! In-memory compact block cache for `zcash_client_backend::sync::run`. Blocks are only held
//! between download and scan; the wallet database is what persists.

use std::{collections::BTreeMap, convert::Infallible, sync::Mutex};

use async_trait::async_trait;
use zcash_client_backend::{
    data_api::{
        chain::{error, BlockCache, BlockSource},
        scanning::ScanRange,
    },
    proto::compact_formats::CompactBlock,
};
use zcash_protocol::consensus::BlockHeight;

#[derive(Default)]
pub struct MemBlockCache {
    blocks: Mutex<BTreeMap<u32, CompactBlock>>,
}

fn height(b: &CompactBlock) -> u32 {
    b.height as u32
}

impl BlockSource for MemBlockCache {
    type Error = Infallible;

    fn with_blocks<F, WalletErrT>(
        &self,
        from_height: Option<BlockHeight>,
        limit: Option<usize>,
        mut with_block: F,
    ) -> Result<(), error::Error<WalletErrT, Self::Error>>
    where
        F: FnMut(CompactBlock) -> Result<(), error::Error<WalletErrT, Self::Error>>,
    {
        let from = from_height.map(u32::from).unwrap_or(0);
        let blocks: Vec<CompactBlock> = {
            let map = self.blocks.lock().unwrap();
            map.range(from..).take(limit.unwrap_or(usize::MAX)).map(|(_, b)| b.clone()).collect()
        };
        for b in blocks {
            with_block(b)?;
        }
        Ok(())
    }
}

#[async_trait]
impl BlockCache for MemBlockCache {
    fn get_tip_height(&self, range: Option<&ScanRange>) -> Result<Option<BlockHeight>, Self::Error> {
        let map = self.blocks.lock().unwrap();
        let tip = match range {
            Some(r) => {
                let (s, e) = (u32::from(r.block_range().start), u32::from(r.block_range().end));
                map.range(s..e).next_back().map(|(h, _)| *h)
            }
            None => map.keys().next_back().copied(),
        };
        Ok(tip.map(BlockHeight::from_u32))
    }

    async fn read(&self, range: &ScanRange) -> Result<Vec<CompactBlock>, Self::Error> {
        let (s, e) = (u32::from(range.block_range().start), u32::from(range.block_range().end));
        Ok(self.blocks.lock().unwrap().range(s..e).map(|(_, b)| b.clone()).collect())
    }

    async fn insert(&self, compact_blocks: Vec<CompactBlock>) -> Result<(), Self::Error> {
        let mut map = self.blocks.lock().unwrap();
        for b in compact_blocks {
            map.insert(height(&b), b);
        }
        Ok(())
    }

    async fn delete(&self, range: ScanRange) -> Result<(), Self::Error> {
        let (s, e) = (u32::from(range.block_range().start), u32::from(range.block_range().end));
        self.blocks.lock().unwrap().retain(|h, _| !(s..e).contains(h));
        Ok(())
    }
}
