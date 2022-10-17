use crate::{
    signature::{identity::Identity, EcdsaSignature},
    BatchContribution, CeremoniesError, Engine, Transcript,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::instrument;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchTranscript {
    pub transcripts:                  Vec<Transcript>,
    pub participant_ids:              Vec<Identity>,
    pub participant_ecdsa_signatures: Vec<EcdsaSignature>,
}

impl BatchTranscript {
    pub fn new<'a, I>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a (usize, usize)> + 'a,
    {
        Self {
            transcripts:                  iter
                .into_iter()
                .map(|(num_g1, num_g2)| Transcript::new(*num_g1, *num_g2))
                .collect(),
            participant_ids:              vec![Identity::None],
            participant_ecdsa_signatures: vec![EcdsaSignature::empty()],
        }
    }

    /// Creates the start of a new batch contribution.
    #[must_use]
    pub fn contribution(&self) -> BatchContribution {
        BatchContribution {
            contributions:   self
                .transcripts
                .iter()
                .map(Transcript::contribution)
                .collect(),
            ecdsa_signature: None,
        }
    }

    /// Adds a batch contribution to the transcript. The contribution must be
    /// valid.
    #[instrument(level = "info", skip_all, fields(n=contribution.contributions.len()))]
    pub fn verify_add<E: Engine>(
        &mut self,
        contribution: BatchContribution,
        identity: Identity,
    ) -> Result<(), CeremoniesError> {
        // Verify contribution count
        if self.transcripts.len() != contribution.contributions.len() {
            return Err(CeremoniesError::UnexpectedNumContributions(
                self.transcripts.len(),
                contribution.contributions.len(),
            ));
        }

        // Verify contributions in parallel
        self.transcripts
            .par_iter_mut()
            .zip(&contribution.contributions)
            .enumerate()
            .try_for_each(|(i, (transcript, contribution))| {
                transcript
                    .verify::<E>(contribution)
                    .map_err(|e| CeremoniesError::InvalidCeremony(i, e))
            })?;

        // Add contributions
        for (transcript, contribution) in self
            .transcripts
            .iter_mut()
            .zip(contribution.contributions.into_iter())
        {
            transcript.add(contribution);
        }

        self.participant_ids.push(identity);

        Ok(())
    }
}

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench {
    use super::*;
    use crate::{
        bench::{rand_entropy, BATCH_SIZE},
        Arkworks, Both, BLST,
    };
    use criterion::{BatchSize, Criterion};

    pub fn group(criterion: &mut Criterion) {
        #[cfg(feature = "arkworks")]
        bench_verify_add::<Arkworks>(criterion, "arkworks");
        #[cfg(feature = "blst")]
        bench_verify_add::<BLST>(criterion, "blst");
        #[cfg(all(feature = "arkworks", feature = "blst"))]
        bench_verify_add::<Both<Arkworks, BLST>>(criterion, "both");
    }

    fn bench_verify_add<E: Engine>(criterion: &mut Criterion, name: &str) {
        // Create a non-trivial transcript
        let transcript = {
            let mut transcript = BatchTranscript::new(BATCH_SIZE.iter());
            let mut contribution = transcript.contribution();
            contribution.add_entropy::<E>(&rand_entropy()).unwrap();
            transcript.verify_add::<E>(contribution).unwrap();
            transcript
        };

        criterion.bench_function(
            &format!("batch_transcript/{}/verify_add", name),
            move |bencher| {
                bencher.iter_batched(
                    || {
                        (transcript.clone(), {
                            let mut contribution = transcript.contribution();
                            contribution.add_entropy::<E>(&rand_entropy()).unwrap();
                            contribution
                        })
                    },
                    |(mut transcript, contribution)| {
                        transcript.verify_add::<E>(contribution).unwrap();
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
}
