use std::collections::HashMap;

use flatbuffers::{self as fb, WIPOffset};
use libp2p_identity::PeerId;

use super::assignment_fb::{self, Assignment, WorkerId};

pub struct AssignmentBuilder {
    builder: fb::FlatBufferBuilder<'static>,
    files_list_offsets: FileListOffsets,
    all_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    current_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    current_dataset_id_offset: Option<fb::WIPOffset<&'static str>>,
    all_datasets: Vec<fb::WIPOffset<assignment_fb::Dataset<'static>>>,
    worker_assignments: Vec<(
        WorkerId,
        fb::WIPOffset<assignment_fb::WorkerAssignment<'static>>,
    )>,
    last_peer_id: Option<PeerId>,
}

impl AssignmentBuilder {
    pub fn new() -> Self {
        Self {
            builder: flatbuffers::FlatBufferBuilder::new(),
            files_list_offsets: HashMap::new(),
            all_chunks: Vec::new(),
            current_chunks: Vec::new(),
            current_dataset_id_offset: None,
            all_datasets: Vec::new(),
            worker_assignments: Vec::new(),
            last_peer_id: None,
        }
    }

    pub fn new_chunk(&mut self) -> ChunkBuilder<'_> {
        ChunkBuilder::new(self)
    }

    pub fn finish_dataset(&mut self) {
        let chunks = self.builder.create_vector(&self.current_chunks);
        let offset = assignment_fb::Dataset::create(
            &mut self.builder,
            &assignment_fb::DatasetArgs {
                id: self.current_dataset_id_offset.take(),
                chunks: Some(chunks),
            },
        );
        self.all_datasets.push(offset);
        self.current_chunks.clear();
    }

    pub fn add_worker(
        &mut self,
        id: PeerId,
        status: assignment_fb::WorkerStatus,
        chunk_indexes: &[u32],
    ) {
        if let Some(last) = self.last_peer_id {
            assert!(
                last < id,
                "Workers must be added in ascending order of their PeerIDs"
            );
        }
        self.last_peer_id = Some(id);

        let worker_id = WorkerId::from(id);
        let chunks = self
            .builder
            .create_vector_from_iter(chunk_indexes.iter().map(|&i| self.all_chunks[i as usize]));
        let offset = assignment_fb::WorkerAssignment::create(
            &mut self.builder,
            &assignment_fb::WorkerAssignmentArgs {
                worker_id: Some(&worker_id),
                chunks: Some(chunks),
                status,
                encrypted_headers: None, // TODO: add encrypted headers
            },
        );
        self.worker_assignments.push((worker_id, offset));
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let datasets = self.builder.create_vector(&self.all_datasets);

        let workers = self
            .builder
            .create_vector_from_iter(self.worker_assignments.iter().map(|(_, offset)| *offset));

        let assignment = Assignment::create(
            &mut self.builder,
            &assignment_fb::AssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
            },
        );

        self.builder.finish(assignment, None);
        self.builder.finished_data().to_vec()
    }

    fn add_chunk(&mut self, offset: fb::WIPOffset<assignment_fb::Chunk<'static>>, dataset: WIPOffset<&'static str>) {
        self.all_chunks.push(offset);
        self.current_chunks.push(offset);
        self.current_dataset_id_offset = Some(dataset);
    }

    fn cache_files_list(
        &mut self,
        files: &[String],
    ) -> fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>
    {
        match self.files_list_offsets.get(files) {
            Some(&offset) => return offset,
            None => {
                let file_offsets: Vec<_> = files
                    .iter()
                    .map(|f| {
                        let name = self.builder.create_string(f);
                        assignment_fb::FileUrl::create(
                            &mut self.builder,
                            &assignment_fb::FileUrlArgs {
                                filename: Some(name),
                                url: Some(name),
                            },
                        )
                    })
                    .collect();
                let offset = self.builder.create_vector(&file_offsets);
                self.files_list_offsets.insert(files.to_vec(), offset);
                offset
            }
        }
    }
}

type FileListOffsets = HashMap<
    Vec<String>,
    fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>,
>;

pub struct ChunkBuilder<'b> {
    p: &'b mut AssignmentBuilder,

    first_block: Option<u64>,
    id: Option<fb::WIPOffset<&'static str>>,
    dataset_id: Option<fb::WIPOffset<&'static str>>,
    size: Option<u32>,
    last_block_hash: Option<fb::WIPOffset<&'static str>>,
    last_block_timestamp: Option<u64>,
    dataset_base_url: Option<fb::WIPOffset<&'static str>>,
    files: Option<
        fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>,
    >,
    worker_indexes: Option<fb::WIPOffset<fb::Vector<'static, u16>>>,
}

impl<'b> ChunkBuilder<'b> {
    pub fn new(parent: &'b mut AssignmentBuilder) -> Self {
        Self {
            p: parent,
            first_block: None,
            id: None,
            dataset_id: None,
            size: None,
            last_block_hash: None,
            last_block_timestamp: None,
            dataset_base_url: None,
            files: None,
            worker_indexes: None,
        }
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(self.p.builder.create_string(&id));
        self
    }

    pub fn dataset_id(mut self, dataset_id: &str) -> Self {
        self.dataset_id = Some(self.p.builder.create_shared_string(dataset_id));
        self
    }

    pub fn first_block(mut self, block: u64) -> Self {
        self.first_block = Some(block);
        self
    }

    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    pub fn last_block_hash(mut self, hash: &str) -> Self {
        self.last_block_hash = Some(self.p.builder.create_string(&hash));
        self
    }

    pub fn last_block_timestamp(mut self, timestamp: u64) -> Self {
        self.last_block_timestamp = Some(timestamp);
        self
    }

    pub fn dataset_base_url(mut self, url: &str) -> Self {
        self.dataset_base_url = Some(self.p.builder.create_shared_string(&url));
        self
    }

    pub fn worker_indexes(mut self, indexes: &[u16]) -> Self {
        self.worker_indexes = Some(self.p.builder.create_vector(indexes));
        self
    }

    pub fn files(mut self, files: &[String]) -> Self {
        self.files = Some(self.p.cache_files_list(files));
        self
    }

    pub fn finish(self) {
        let offset = assignment_fb::Chunk::create(
            &mut self.p.builder,
            &assignment_fb::ChunkArgs {
                id: self.id,
                first_block: self.first_block.expect("First block must be set"),
                last_block_hash: self.last_block_hash,
                last_block_timestamp: self.last_block_timestamp,
                dataset_id: self.dataset_id,
                size: self.size.expect("Size must be set"),
                dataset_base_url: self.dataset_base_url,
                base_url: self.id,
                files: self.files,
                worker_indexes: self.worker_indexes,
            },
        );
        self.p.add_chunk(offset, self.dataset_id.expect("Dataset ID must be set"));
    }
}
