```rs
fn worker_thread(client: Arc<Client>, mut peer: Peer, work_queue: Arc<WorkQueue>) {
    loop {
        let Some(piece_index) = work_queue.get_work() else {
            break;
        };

        match client.download_piece_from_peer(&mut peer, piece_index) {
            Ok(piece) => {
                // Verify hash.
                // Write piece to output.
            }
            Err(_) => {
                work_queue.return_work(piece_index);

                // Get another peer.
            }
        }
    }
}
```

```rs
    fn start_work_pool(
        &self,
        work_queue: Arc<WorkQueue>,
        peers: Vec<Peer>,
        output: &Path,
    ) -> io::Result<()> {
        thread::scope(|scope| {
            let mut handles = Vec::new();

            for mut peer in peers {
                let work_queue = Arc::clone(&work_queue);

                let handle = scope.spawn(move || -> io::Result<()> {
                    loop {
                        let Some(piece_index) = work_queue.get_work() else {
                            break;
                        };

                        let piece_length = self.piece_length(piece_index);

                        match self.download_piece_from_peer(
                            &mut peer,
                            piece_index,
                            piece_length,
                            16 * 1024,
                        ) {
                            Ok(piece) if self.verify_piece(piece_index, &piece) => {
                                self.write_temp_piece(output, piece_index, &piece)?;
                                work_queue.complete_work();
                            }

                            _ => {
                                work_queue.return_work(piece_index);
                                break;
                            }
                        }
                    }

                    Ok(())
                });

                handles.push(handle);
            }

            for handle in handles {
                handle
                    .join()
                    .map_err(|_| io::Error::other("Worker panicked"))??;
            }

            Ok(())
        })
    }
```

```rs
    pub fn download_file(&self, output: &Path) -> io::Result<()> {
        // Fetch peers and establish up to 8 connections.

        let work_queue = Arc::new(WorkQueue::new(piece_count));

        self.start_work_pool(work_queue, peers, output)?;

        assemble_file(output, piece_count)?;

        Ok(())
    }
```

loop?

- download_piece_from_peer might fails
- if it fails, find_next_active_peer
- and download_piece_from_peer again

how to match with workqueue:

- outerloop: find_next_active_peer
- innerloop: fetch piece_index and call download_piece_from_peer
  - if failed, find_next_active_peer and try again on the same piece_index
  - add jitter for delay before trying
