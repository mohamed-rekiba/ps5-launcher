# The Secure Boot signing key

The NVIDIA image's kernel modules are signed with PS5 Launcher OS's own key. A PC with Secure
Boot on loads them only after the user enrolls the key's certificate once, on the blue MOK
screen. Both images ship the certificate at
`/usr/share/ps5-launcher/secureboot/<sha256>.der` and carry its SHA-256 in the label
`io.github.ps5-launcher.secureboot-cert-sha256`.

| What | Where |
|---|---|
| Private key (PEM) | The owner's safe backup, and the GitHub Actions secret `SECUREBOOT_KEY`. **Never in the repository.** |
| Certificate (DER) | `packaging/os/secureboot/public_key.der`, committed. |

Until `public_key.der` exists, every image build fails with a message that points here. The
workflow `.github/workflows/os.yml` checks that the secret and the certificate belong
together before it builds anything, and mounts the key only for the signing step.

## Make the key (the project owner, once)

Do this on your own trusted machine, not on a shared or CI machine. You need OpenSSL 3 or newer
(`openssl version`; macOS ships LibreSSL as `/usr/bin/openssl`, so use Homebrew's `openssl@3`)
and the GitHub CLI (`gh`), logged in to the repository.

1. Make the key and the certificate: RSA-3072, SHA-256, self-signed X.509, for code signing,
   valid for 10 years. Run this outside the repository, so the private key never lands in it:

   ```bash
   mkdir -m 700 ~/ps5-launcher-secureboot && cd ~/ps5-launcher-secureboot
   openssl req -new -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
       -subj "/CN=PS5 Launcher OS/" \
       -addext "extendedKeyUsage=codeSigning" \
       -keyout private_key.pem -outform DER -out public_key.der
   chmod 600 private_key.pem
   ```

2. Check it:

   ```bash
   openssl x509 -inform DER -in public_key.der -noout -subject -dates -ext extendedKeyUsage
   ```

   It must show `CN=PS5 Launcher OS`, about 10 years between the two dates, and
   `Code Signing`.

3. Back up `private_key.pem` safely before you go on: for example in a password manager, or on
   an encrypted USB drive kept offline. **If this key is lost, every NVIDIA PC must enroll a new
   key.** If it leaks, anyone can sign kernel modules that those PCs trust. Keep
   `public_key.der` with the backup.

4. Store the private key as the repository secret. `gh` reads it from standard input, so it is
   not in your shell history:

   ```bash
   gh secret set SECUREBOOT_KEY --repo MohamedAliRashad/ps5-launcher < private_key.pem
   ```

5. Commit **only** the certificate:

   ```bash
   cp public_key.der /path/to/ps5-launcher/packaging/os/secureboot/public_key.der
   cd /path/to/ps5-launcher
   git add packaging/os/secureboot/public_key.der
   git commit -m "feat: add the Secure Boot certificate for the NVIDIA image"
   ```

6. Keep the folder `~/ps5-launcher-secureboot` on an encrypted disk, or delete it once the
   backup and the secret are in place.

## Rotate the key

Not needed for 10 years. When it is: make a new key the same way, and ship the old and the new
certificate side by side for at least one release (each under its own SHA-256 name), so the
helper can queue the new one before it stages an image signed with it. The OS plan describes
the rules ("Secure Boot key lifecycle").

## Test builds

`packaging/os/make-test-key.sh` makes a throwaway key in `target/os/test-key/` for local builds:
set `SECUREBOOT_CERT_FILE` and, for the NVIDIA image, `SECUREBOOT_KEY_FILE`. Never publish an
image built with it.
