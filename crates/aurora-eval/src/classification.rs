use std::collections::BTreeMap;

use crate::{ClassMetric, MetricCount};

pub(crate) struct ClassificationCounts<L> {
    pub(crate) accuracy: MetricCount,
    pub(crate) missing_predictions: u64,
    pub(crate) classes: BTreeMap<L, ClassMetric>,
}

pub(crate) fn score_classes<L>(
    universe: &[L],
    samples: impl IntoIterator<Item = (L, Option<L>)>,
) -> ClassificationCounts<L>
where
    L: Copy + Ord,
{
    let mut counts = universe
        .iter()
        .copied()
        .map(|label| (label, MutableClassMetric::default()))
        .collect::<BTreeMap<_, _>>();
    let mut correct = 0_u64;
    let mut total = 0_u64;
    let mut missing = 0_u64;
    for (expected, predicted) in samples {
        total += 1;
        match predicted {
            Some(predicted) if predicted == expected => {
                correct += 1;
                counts
                    .get_mut(&expected)
                    .expect("the class universe contains every expected label")
                    .true_positive += 1;
            }
            Some(predicted) => {
                counts
                    .get_mut(&expected)
                    .expect("the class universe contains every expected label")
                    .false_negative += 1;
                counts
                    .get_mut(&predicted)
                    .expect("the class universe contains every predicted label")
                    .false_positive += 1;
            }
            None => {
                missing += 1;
                counts
                    .get_mut(&expected)
                    .expect("the class universe contains every expected label")
                    .false_negative += 1;
            }
        }
    }
    ClassificationCounts {
        accuracy: MetricCount::new(correct, total),
        missing_predictions: missing,
        classes: counts
            .into_iter()
            .map(|(label, metric)| (label, metric.finish()))
            .collect(),
    }
}

#[derive(Default)]
struct MutableClassMetric {
    true_positive: u64,
    false_positive: u64,
    false_negative: u64,
}

impl MutableClassMetric {
    fn finish(self) -> ClassMetric {
        ClassMetric::new(self.true_positive, self.false_positive, self.false_negative)
    }
}

#[cfg(test)]
mod tests {
    use super::score_classes;
    use proptest::prelude::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Label {
        A,
        B,
    }

    #[test]
    fn missing_and_wrong_predictions_conserve_labelled_support() {
        let scored = score_classes(
            &[Label::A, Label::B],
            [
                (Label::A, Some(Label::A)),
                (Label::A, Some(Label::B)),
                (Label::A, None),
                (Label::B, Some(Label::B)),
            ],
        );
        let a = scored.classes.get(&Label::A).expect("class exists");

        assert_eq!(scored.accuracy.matched(), 2);
        assert_eq!(scored.accuracy.total(), 4);
        assert_eq!(scored.missing_predictions, 1);
        assert_eq!(a.true_positive(), 1);
        assert_eq!(a.false_negative(), 2);
        assert_eq!(a.support(), 3);
    }

    proptest! {
        #[test]
        fn generated_classification_counts_conserve_examples(
            samples in prop::collection::vec((any::<bool>(), prop::option::of(any::<bool>())), 0..100)
        ) {
            let mapped = samples.iter().map(|(expected, predicted)| {
                (
                    if *expected { Label::A } else { Label::B },
                    predicted.map(|value| if value { Label::A } else { Label::B }),
                )
            });
            let scored = score_classes(&[Label::A, Label::B], mapped);
            let support = scored.classes.values().map(|metric| metric.support()).sum::<u64>();
            let true_positives = scored
                .classes
                .values()
                .map(|metric| metric.true_positive())
                .sum::<u64>();
            let false_positives = scored
                .classes
                .values()
                .map(|metric| metric.false_positive())
                .sum::<u64>();

            prop_assert_eq!(support, samples.len() as u64);
            prop_assert_eq!(true_positives, scored.accuracy.matched());
            prop_assert_eq!(
                false_positives + scored.missing_predictions,
                scored.accuracy.total() - scored.accuracy.matched()
            );
        }
    }
}
