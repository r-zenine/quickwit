// Copyright (C) 2021 Quickwit, Inc.
//
// Quickwit is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at hello@quickwit.io.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

use std::ops::RangeInclusive;
use std::str::FromStr;

use crate::postgresql::schema::{indexes, splits};
use crate::{IndexMetadata, SplitMetadataAndFooterOffsets, SplitState};

/// A model structure for handling index metadata in a database.
#[derive(Identifiable, Insertable, Queryable, Debug)]
#[primary_key(index_id)]
#[table_name = "indexes"]
pub struct Index {
    /// Index ID. The index ID identifies the index when querying the metastore.
    pub index_id: String,
    // A JSON string containing all of the IndexMetadata.
    pub index_metadata_json: String,
}

impl Index {
    /// Make IndexMetadata from stored JSON string.
    pub fn make_index_metadata(&self) -> anyhow::Result<IndexMetadata> {
        let index_metadata =
            serde_json::from_str::<IndexMetadata>(self.index_metadata_json.as_str())
                .map_err(|err| anyhow::anyhow!(err))?;

        Ok(index_metadata)
    }
}

/// A model structure for handling split metadata in a database.
#[derive(Identifiable, Insertable, Associations, Queryable, Debug)]
#[belongs_to(Index)]
#[primary_key(split_id)]
#[table_name = "splits"]
pub struct Split {
    /// Split ID.
    pub split_id: String,
    /// The state of the split. This is the only mutable attribute of the split.
    pub split_state: String,
    /// If a timestamp field is available, the min timestamp in the split.
    pub start_time_range: Option<i64>,
    /// If a timestamp field is available, the max timestamp in the split.
    pub end_time_range: Option<i64>,
    /// A list of tags for categorizing and searching group of splits.
    pub tags: Vec<String>,
    // A JSON string containing all of the SplitMetadataAndFooterOffsets.
    pub split_metadata_json: String,
    /// Index ID. It is used as a foreign key in the database.
    pub index_id: String,
}

impl Split {
    /// Make time range from start_time_range and end_time_range in database model.
    pub fn get_time_range(&self) -> Option<RangeInclusive<i64>> {
        self.start_time_range.and_then(|start_time_range| {
            self.end_time_range
                .map(|end_time_range| RangeInclusive::new(start_time_range, end_time_range))
        })
    }

    /// Get split state from split_state in database model.
    pub fn get_split_state(&self) -> Option<SplitState> {
        SplitState::from_str(&self.split_state).ok()
    }

    /// Make SplitMetadataAndFooterOffsets from stored JSON string.
    pub fn make_split_metadata_and_footer_offsets(
        &self,
    ) -> anyhow::Result<SplitMetadataAndFooterOffsets> {
        let split_metadata_and_fotter_offsets =
            serde_json::from_str::<SplitMetadataAndFooterOffsets>(
                self.split_metadata_json.as_str(),
            )
            .map_err(|err| anyhow::anyhow!(err))?;

        Ok(split_metadata_and_fotter_offsets)
    }
}
