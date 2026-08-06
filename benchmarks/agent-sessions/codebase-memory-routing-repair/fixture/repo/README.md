# Delivery router fixture

This small crate models production ordered-delivery routing. Topic aliases must
share worker affinity with their canonical topic across first and retry
attempts. Tenant identity remains part of the affinity key.
