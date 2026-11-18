use std::sync::{Condvar, Mutex};

struct QueueState {
    pieces: Vec<usize>,
    completed: usize,
}

pub struct WorkQueue {
    state: Mutex<QueueState>,
    condvar: Condvar,
    total_pieces: usize,
}

impl WorkQueue {
    pub fn new(piece_count: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                pieces: (0..piece_count).collect(),
                completed: 0,
            }),
            condvar: Condvar::new(),
            total_pieces: piece_count,
        }
    }

    pub fn get_work(&self) -> Option<usize> {
        loop {
            let mut state = self.state.lock().unwrap();
            if state.completed == self.total_pieces {
                return None;
            }

            if let Some(piece_index) = state.pieces.pop() {
                return Some(piece_index);
            }

            state = self.condvar.wait(state).unwrap();
        }
    }

    pub fn complete_work(&self) {
        let mut state = self.state.lock().unwrap();

        state.completed += 1;

        if state.completed == self.total_pieces {
            self.condvar.notify_all();
        }
    }

    pub fn return_work(&self, piece_index: usize) {
        let mut state = self.state.lock().unwrap();

        state.pieces.push(piece_index);

        self.condvar.notify_one();
    }
}
