use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use runx_parser::GraphStep;

use super::RUNX_MAX_FANOUT_CONCURRENCY_ENV;
use crate::effects::RuntimeEffectRegistry;

const HARD_MAX_FANOUT_CONCURRENCY: usize = 64;

pub(super) struct FanoutScheduler {
    max_concurrency: usize,
}

pub(super) enum FanoutSchedule<T> {
    Serial(Vec<T>),
    Parallel(ParallelFanoutSchedule<T>),
}

pub(super) struct ParallelFanoutSchedule<T> {
    pub(super) steps: Vec<T>,
    pub(super) max_concurrency: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParallelWidth {
    Bounded(NonZeroUsize),
    Unbounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Parallelism {
    Serial,
    Isolated(ParallelWidth),
}

impl FanoutScheduler {
    pub(super) fn from_env(env: &BTreeMap<String, String>) -> Self {
        Self {
            max_concurrency: configured_max_concurrency(env),
        }
    }

    pub(super) fn can_parallelize(&self, step_count: usize) -> bool {
        self.max_concurrency > 1 && step_count > 1
    }

    pub(super) fn schedule<T>(
        &self,
        steps: Vec<T>,
        parallelism: impl Fn(&T) -> Parallelism,
    ) -> FanoutSchedule<T> {
        if self.max_concurrency <= 1 || steps.len() <= 1 {
            return FanoutSchedule::Serial(steps);
        }
        let mut schedule_limit = self.max_concurrency;
        for step in &steps {
            match parallelism(step) {
                Parallelism::Serial => return FanoutSchedule::Serial(steps),
                Parallelism::Isolated(ParallelWidth::Bounded(width)) => {
                    schedule_limit = schedule_limit.min(width.get());
                }
                Parallelism::Isolated(ParallelWidth::Unbounded) => {}
            }
        }
        FanoutSchedule::Parallel(ParallelFanoutSchedule {
            steps,
            max_concurrency: schedule_limit,
        })
    }
}

pub(super) fn parallel_safe_step_shape(step: &GraphStep, effects: &RuntimeEffectRegistry) -> bool {
    step.skill.is_some() && effects.allows_parallel_step(step)
}

pub(super) fn configured_max_concurrency(env: &BTreeMap<String, String>) -> usize {
    env.get(RUNX_MAX_FANOUT_CONCURRENCY_ENV)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(default_max_fanout_concurrency)
        .min(HARD_MAX_FANOUT_CONCURRENCY)
}

fn default_max_fanout_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .map_or(1, cap_platform_parallelism)
}

fn cap_platform_parallelism(available: usize) -> usize {
    available.clamp(1, HARD_MAX_FANOUT_CONCURRENCY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_available_bounded_parallelism() {
        assert_eq!(
            configured_max_concurrency(&BTreeMap::new()),
            default_max_fanout_concurrency()
        );
        assert_eq!(cap_platform_parallelism(8), 8);
        assert_eq!(
            cap_platform_parallelism(usize::MAX),
            HARD_MAX_FANOUT_CONCURRENCY
        );
    }

    #[test]
    fn clamps_configured_fanout_concurrency() {
        let mut env = BTreeMap::new();
        env.insert(
            RUNX_MAX_FANOUT_CONCURRENCY_ENV.to_owned(),
            "100000".to_owned(),
        );
        assert_eq!(
            configured_max_concurrency(&env),
            HARD_MAX_FANOUT_CONCURRENCY
        );
    }

    #[test]
    fn keeps_mixed_capability_fanout_serial() {
        let scheduler = FanoutScheduler {
            max_concurrency: HARD_MAX_FANOUT_CONCURRENCY,
        };
        let steps = vec![
            Parallelism::Isolated(ParallelWidth::Unbounded),
            Parallelism::Serial,
        ];
        assert!(matches!(
            scheduler.schedule(steps, |step| *step),
            FanoutSchedule::Serial(_)
        ));
    }

    #[test]
    fn clamps_parallel_schedule_to_the_narrowest_adapter_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let scheduler = FanoutScheduler {
            max_concurrency: HARD_MAX_FANOUT_CONCURRENCY,
        };
        let four = NonZeroUsize::new(4).ok_or("fixture width must be non-zero")?;
        let eight = NonZeroUsize::new(8).ok_or("fixture width must be non-zero")?;
        let schedule = scheduler.schedule(
            vec![
                Parallelism::Isolated(ParallelWidth::Bounded(four)),
                Parallelism::Isolated(ParallelWidth::Bounded(eight)),
            ],
            |step| *step,
        );
        assert!(matches!(
            schedule,
            FanoutSchedule::Parallel(ParallelFanoutSchedule {
                max_concurrency: 4,
                ..
            })
        ));
        Ok(())
    }
}
