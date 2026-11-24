use crate::model::{Features, Model, OUTPUTS, Prediction};
use std::{
    ops::Range,
    sync::{
        Arc, Mutex, RwLock,
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
};

pub struct Scalar<'a> {
    model: &'a Model,
    hidden_a: Vec<f32>,
    hidden_b: Vec<f32>,
}

impl<'a> Scalar<'a> {
    pub fn new(model: &'a Model) -> Self {
        Self {
            model,
            hidden_a: vec![0.0; model.layers[0].outputs],
            hidden_b: vec![0.0; model.layers[1].outputs],
        }
    }

    pub fn infer(&mut self, features: &Features) -> Prediction {
        let mut out = [0.0; OUTPUTS];
        self.model.layers[0].forward_rows(features, 0, &mut self.hidden_a);
        self.model.layers[1].forward_rows(&self.hidden_a, 0, &mut self.hidden_b);
        self.model.layers[2].forward_rows(&self.hidden_b, 0, &mut out);
        out
    }
}

pub struct Parallel {
    shared: Arc<Shared>,
    workers: Vec<Worker>,
}

struct Shared {
    model: Arc<Model>,
    input: RwLock<Vec<f32>>,
    chunks: Vec<Mutex<Vec<f32>>>,
}

struct Worker {
    start: Option<SyncSender<usize>>,
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Parallel {
    pub fn new(model: Arc<Model>, threads: usize) -> Result<Self, String> {
        let input_len = model.layers.iter().map(|layer| layer.inputs).max().unwrap();
        let output_len = model
            .layers
            .iter()
            .map(|layer| layer.outputs)
            .max()
            .unwrap();
        let shared = Arc::new(Shared {
            model,
            input: RwLock::new(vec![0.0; input_len]),
            chunks: (0..threads)
                .map(|_| Mutex::new(vec![0.0; output_len.div_ceil(threads)]))
                .collect(),
        });
        let mut parallel = Self {
            shared,
            workers: Vec::new(),
        };
        for worker in 1..threads {
            let shared = Arc::clone(&parallel.shared);
            let (start, jobs) = sync_channel(1);
            let (finished, done) = sync_channel(1);
            let thread = thread::Builder::new()
                .name(format!("mlp-cpu-{worker}"))
                .spawn(move || {
                    while let Ok(layer) = jobs.recv() {
                        shared.forward(layer, worker);
                        if finished.send(()).is_err() {
                            return;
                        }
                    }
                })
                .map_err(|e| format!("spawn CPU worker: {e}"))?;
            parallel.workers.push(Worker {
                start: Some(start),
                done,
                thread: Some(thread),
            });
        }
        Ok(parallel)
    }

    pub fn infer(&mut self, features: &Features) -> Result<Prediction, String> {
        self.shared.input.write().unwrap()[..features.len()].copy_from_slice(features);
        for (index, layer) in self.shared.model.layers.iter().enumerate() {
            for worker in &self.workers {
                worker
                    .start
                    .as_ref()
                    .unwrap()
                    .send(index)
                    .map_err(|e| format!("start dense layer: {e}"))?;
            }
            self.shared.forward(index, 0);
            for worker in &self.workers {
                worker
                    .done
                    .recv()
                    .map_err(|e| format!("finish dense layer: {e}"))?;
            }
            let mut input = self.shared.input.write().unwrap();
            for (worker, chunk) in self.shared.chunks.iter().enumerate() {
                let rows = rows_for(layer.outputs, worker, self.shared.chunks.len());
                let chunk = chunk.lock().unwrap();
                let len = rows.len();
                input[rows].copy_from_slice(&chunk[..len]);
            }
        }
        Ok(self.shared.input.read().unwrap()[..OUTPUTS]
            .try_into()
            .unwrap())
    }
}

impl Drop for Parallel {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            worker.start.take();
            let _ = worker.thread.take().unwrap().join();
        }
    }
}

impl Shared {
    fn forward(&self, index: usize, worker: usize) {
        let layer = &self.model.layers[index];
        let rows = rows_for(layer.outputs, worker, self.chunks.len());
        let input = self.input.read().unwrap();
        let mut output = self.chunks[worker].lock().unwrap();
        layer.forward_rows(
            &input[..layer.inputs],
            rows.start,
            &mut output[..rows.len()],
        );
    }
}

fn rows_for(outputs: usize, worker: usize, threads: usize) -> Range<usize> {
    outputs * worker / threads..outputs * (worker + 1) / threads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::features;

    #[test]
    fn parallel_rows_match_scalar_for_uneven_chunks_and_consecutive_inputs() {
        let model = Arc::new(Model::new(71, 42));
        let mut scalar = Scalar::new(&model);
        let mut parallel = Parallel::new(Arc::clone(&model), 4).unwrap();
        for input in features(42) {
            assert_eq!(parallel.infer(&input).unwrap(), scalar.infer(&input));
        }
    }
}
