#!/usr/bin/env bash
# Make throwaway keys for test builds of the images. TESTS ONLY: never publish an image built
# with them. The release keys are made once by the project owner and never stored in the
# repository: the Secure Boot key (packaging/os/secureboot/README.md) and the image signing key
# (packaging/os/signing/README.md).
#
#   packaging/os/make-test-key.sh    # writes target/os/test-key/{private_key.priv,public_key.der}
#                                    # and the public half of a signing key, cosign.pub
set -euo pipefail
dir=target/os/test-key
mkdir -p "$dir"
openssl req -new -x509 -newkey rsa:3072 -nodes -days 3650 -sha256 \
    -subj "/CN=PS5 Launcher OS test key/" \
    -addext "extendedKeyUsage=codeSigning" \
    -keyout "$dir/private_key.priv" -outform DER -out "$dir/public_key.der"
chmod 600 "$dir/private_key.priv"
# An ECDSA P-256 public key in PEM, the format of cosign.pub. Only its public half is kept: a test
# image ships it in its policy, and nothing is signed with it.
openssl ecparam -name prime256v1 -genkey -noout | openssl ec -pubout -out "$dir/cosign.pub" 2>/dev/null
echo "Build with: SECUREBOOT_CERT_FILE=$dir/public_key.der SIGNING_PUBKEY_FILE=$dir/cosign.pub, and for nvidia SECUREBOOT_KEY_FILE=$dir/private_key.priv"
