package pcloud.policy

# Safe baseline: every request that isn't explicitly allowed by another
# policy file is rejected with a clear reason.
default decision = {"allow": false, "reason": "default deny"}
