package pcloud.policy

import future.keywords.if

# Default deny so unrelated commands are not implicitly allowed by this file.
default decision = {"allow": false, "reason": "managed-device policy: not allowed"}

# Allow crypto.setup only from devices whose ID starts with "managed-".
decision = {"allow": true} if {
    input.command == "crypto.setup"
    startswith(input.device_id, "managed-")
}
