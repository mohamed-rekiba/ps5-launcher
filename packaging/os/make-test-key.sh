#!/usr/bin/env bash
# Make throwaway keys for test builds of the image. TESTS ONLY: never publish an image built
# with them. The release keys are made once by the project owner and never stored in the
# repository: the image signing key (packaging/os/signing/README.md).
# Writes target/os/test-key/cosign.pub for local checks only.
set -euo pipefail
dir=target/os/test-key
mkdir -p "$dir"

# An ECDSA P-256 public key in PEM, the format of cosign.pub. Only its public half is kept: a test
# image ships it in its policy, and nothing is signed with it.
openssl ecparam -name prime256v1 -genkey -noout | openssl ec -pubout -out "$dir/cosign.pub" 2>/dev/null
echo "Build with: SIGNING_PUBKEY_FILE=$dir/cosign.pub"
