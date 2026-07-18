use super::*;

pub(super) fn collect_backend_signal_evidence(
    kernel: &mut dyn ContainmentKernel,
    evidence: &mut CleanupEvidence,
) {
    collect_one(kernel, evidence, ContainmentSignal::Term);
    collect_one(kernel, evidence, ContainmentSignal::Kill);
}

fn collect_one(
    kernel: &mut dyn ContainmentKernel,
    evidence: &mut CleanupEvidence,
    signal: ContainmentSignal,
) {
    let Some(batch) = kernel.take_backend_signal_batch(signal) else {
        return;
    };
    let survivors: Vec<_> = batch
        .attempts()
        .iter()
        .map(|attempt| attempt.process().clone())
        .collect();
    evidence.remember_survivors(&survivors, batch.omitted());
    evidence.remember_batch(signal, &batch);
    evidence.term_attempted = true;
    evidence.disposition = match signal {
        ContainmentSignal::Term => CleanupDisposition::Terminated,
        ContainmentSignal::Kill => CleanupDisposition::Killed,
    };
}
