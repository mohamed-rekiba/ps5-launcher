#!/usr/bin/env bash
# Make a throwaway Secure Boot key for test builds of the images. TESTS ONLY: never publish an
# image built with it. The release key is made once by the project owner, kept as the
# SECUREBOOT_KEY secret, and never stored in the repository (packaging/os/secureboot/README.md).
#
#   packaging/os/make-test-key.sh    # writes target/os/test-key/{private_key.priv,public_key.der}
set -euo pipefail
dir=target/os/test-key
mkdir -p "$dir"
openssl req -new -x509 -newkey rsa:3072 -nodes -days 3650 -sha256 \
    -subj "/CN=PS5 Launcher OS test key/" \
    -addext "extendedKeyUsage=codeSigning" \
    -keyout "$dir/private_key.priv" -outform DER -out "$dir/public_key.der"
chmod 600 "$dir/private_key.priv"
echo "Build with: SECUREBOOT_CERT_FILE=$dir/public_key.der, and for nvidia SECUREBOOT_KEY_FILE=$dir/private_key.priv"
