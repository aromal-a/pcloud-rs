package pcloud.policy

import future.keywords.if

# Allow by default; deny only the specific case we care about.
default decision = {"allow": true}

# Deny publink.create when the requested expiry exceeds 7 days.
# `input.args.expiry_days` is supplied by the daemon as an integer.
decision = {"allow": false, "reason": "publink expiry exceeds 7 days"} if {
    input.command == "publink.create"
    input.args.expiry_days > 7
}
