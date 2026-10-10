# The image signing key

Every PS5 Launcher OS image is signed with the project's own cosign key, separate from the
Secure Boot key. Installed PCs pull an image only when its signature by this key checks out:
bootc, the launcher's root helper and the first start all use the containers policy that both
images ship.

| What | Where |
|---|---|
| Private key (cosign's encrypted PEM, `cosign.key`) | The owner's safe backup, and the secret `OS_IMAGE_SIGNING_KEY` of the environment `os-fedora-signing`. **Never in the repository.** |
| Its password | The owner's safe backup, and the secret `COSIGN_PASSWORD` of the same environment. |
| Public key (`cosign.pub`, PEM) | `packaging/os/signing/cosign.pub`, committed. Both images ship it as `/usr/share/ps5-launcher/signing/cosign.pub`. |

Until `cosign.pub` exists, every run of `.github/workflows/os.yml` and every image build
fails with a message that points here. The workflow's `sign` job checks that the secret is the
private half of `cosign.pub` before it signs anything.

## What the images ship

Both images get these from the templates in this folder, with the image's own repository
(`ghcr.io/<owner>/ps5-launcher-fedora`, the name part of `IMAGE_REF`) in place of
`@REPOSITORY@`:

- `/etc/containers/policy.json` (from [policy.json.in](policy.json.in)): `default` is `reject`.
  Under `docker`, only our repository is allowed, and only with a sigstore signature by
  `/usr/share/ps5-launcher/signing/cosign.pub`, with `signedIdentity` `matchRepository`. cosign
  writes the repository name without a tag as the signature's identity, and containers/image's
  default identity rule refuses a name without a tag or digest, so `matchRepository` is needed.
  The local transports (`containers-storage`, `docker-daemon`, `dir`, `oci`, `oci-archive`,
  `docker-archive`) accept anything.
- `/etc/containers/registries.d/ps5-launcher.yaml` (from [registries.yaml.in](registries.yaml.in)):
  `use-sigstore-attachments: true` for our repository, so the signature is read from the tag
  `sha256-<digest>.sig` next to the image.

**What `default: reject` refuses, and why that is fine.** On an installed PC, `podman pull`,
`toolbox create` and anything else that pulls through containers/image from a registry other
than ours is refused. The OS needs none of them: bootc pulls only our repository, and the launcher
runs no containers. A user who wants podman or toolbox can give their own account its own policy
in `~/.config/containers/policy.json`; containers/image reads that file first for that user, and
the system's policy for root (bootc, the helper) stays as it is.

**Why the local transports stay allowed.** `bootc install` run from the image itself (CI's boot
test makes its disk with image-builder that way) reads the image from `containers-storage:` under
the image's own policy. A `reject` there would break it. Local images are already on the PC, so
the policy has nothing to protect there.

**The installer** is not affected: Fedora's installer runs `bootc install to-filesystem` with
its own containers policy (Anaconda 44.30, `pyanaconda/modules/payloads/payload/rpm_ostree/installation.py`,
`DeployBootcTask`), which checks no signature. That is why the release ISO installs a digest, never
a tag, and why the first start turns the enforcement on (see
[../README.md](../README.md#image-signatures-on-the-installed-pc)).

## Make the key (the project owner, once)

Do this on your own trusted machine, not on a shared or CI machine. You need cosign v3.1.3 (the
version the workflow pins; `cosign version`) and the GitHub CLI (`gh`), logged in to the
repository.

1. Make the key pair, outside the repository, so the private key never lands in it. cosign asks
   for a password twice: use a long random one, and keep it with the key.

   ```bash
   mkdir -m 700 ~/ps5-launcher-signing && cd ~/ps5-launcher-signing
   cosign generate-key-pair
   chmod 600 cosign.key
   ```

   This writes `cosign.key` (the private key, encrypted with the password) and `cosign.pub` (an
   ECDSA P-256 public key in PEM).

2. Check it:

   ```bash
   cosign public-key --key cosign.key | diff - cosign.pub && echo "the pair matches"
   openssl pkey -pubin -in cosign.pub -noout -text | head -1
   ```

   It must print `the pair matches` (after the password), and `Public-Key: (256 bit)`.

3. Back up `cosign.key` and its password safely before you go on: for example in a password
   manager, or on an encrypted USB drive kept offline. **If this key is lost, no new image can be
   signed for the PCs already installed: they need the rotation below, signed with the old key.**
   If it leaks, anyone who can push to the repository on GHCR can make images those PCs accept.
   Keep `cosign.pub` with the backup.

4. Create the environment `os-fedora-signing` (repository Settings → Environments → New
   environment), **before** you add its secrets, and before the workflow first runs: GitHub
   creates an environment the first time a workflow names it, with no rules at all. Under *Deployment branches and tags*, choose *Selected branches and tags* and
   add the branch `main` and the tag pattern `v*` (a published launcher release runs from its
   tag). Add no required reviewers: every run signs its candidates. Protect the `main` branch
   and the `v*` tags (Settings → Rules; see docs/DEVELOPMENT.md, Releases), so only reviewed
   changes reach them.

5. Store the key and the password as secrets **of that environment**. `gh` reads them from
   standard input, so they are not in your shell history:

   ```bash
   gh secret set OS_IMAGE_SIGNING_KEY --env os-fedora-signing --repo MohamedAliRashad/ps5-launcher < cosign.key
   gh secret set COSIGN_PASSWORD --env os-fedora-signing --repo MohamedAliRashad/ps5-launcher
   ```

   The second command asks for the password.

6. Commit **only** the public key:

   ```bash
   cp cosign.pub /path/to/ps5-launcher/packaging/os/signing/cosign.pub
   cd /path/to/ps5-launcher
   git add packaging/os/signing/cosign.pub
   git commit -m "feat: add the image signing public key"
   ```

7. Keep the folder `~/ps5-launcher-signing` on an encrypted disk, or delete it once the backup
   and the secrets are in place.

## The two environments

| Environment | Used by | Its rules (the owner sets them) |
|---|---|---|
| `os-fedora-signing` | the `sign` job | Deployment branches and tags: `main` and `v*`. No reviewers. Secrets `OS_IMAGE_SIGNING_KEY`, `COSIGN_PASSWORD`. |
| `os-fedora-release` | the `promote` job | Deployment branches and tags: `main` and `v*`. Required reviewers, who approve only with hardware results for that exact run. |

The signing key is not in `os-fedora-release`: its required reviewers would hold every daily run
at the `sign` job. A run from any other branch fails at `sign`, so it gets no signed candidates
and no install test; run the workflow from `main`.

## How the workflow signs and checks

- `sign` signs the main, NVIDIA and upgrade-test digests right after they are pushed, with
  `cosign sign --key env://OS_IMAGE_SIGNING_KEY --use-signing-config=false --new-bundle-format=false --tlog-upload=false --registry-referrers-mode=legacy --yes`,
  and the annotations `io.github.ps5-launcher.run-id`, `.run-attempt`, `.commit` and `.variant`.
  These flags make cosign v3.1.3 write the legacy signature (a `sha256-<digest>.sig` tag with
  the "simple signing" payload), which is what containers/image reads, and upload nothing to a
  transparency log (cosign v3.1.3 `cmd/cosign/cli/options/sign.go` lines 125, 161, 164;
  `registry.go` line 221; `cmd/cosign/cli/signcommon/common.go` lines 437-442).
- `promote` checks both digests with `cosign verify --key packaging/os/signing/cosign.pub --insecure-ignore-tlog --new-bundle-format=false`
  and this run's annotations, and with Fedora 44's skopeo copying each digest under the images'
  own policy. After the copy to the release tags it checks each tag's digest and signature
  again. The release tags are in the same repository and keep the digests
  (`skopeo copy --preserve-digests`), so each signature, stored by digest, follows.
- CI only: `test-images` pushes the upgrade-test image unsigned, and signed with a throwaway key
  made in the job. The install test checks that the VM refuses both for `bootc switch` and
  `bootc upgrade`.

## Rotate the key

The bridge: PCs trust only the key their running image ships, so a new key must arrive in an
image signed with the old one.

1. Make the new pair as above (do not overwrite the old one), and commit the new public key
   beside the old one, for example as `cosign-2.pub`, shipped as
   `/usr/share/ps5-launcher/signing/cosign-2.pub`.
2. The bridge release: change `policy.json.in` so our repository's **one** requirement lists
   both keys with `keyPaths`:

   ```json
   {
       "type": "sigstoreSigned",
       "keyPaths": ["/usr/share/ps5-launcher/signing/cosign.pub",
                    "/usr/share/ps5-launcher/signing/cosign-2.pub"],
       "signedIdentity": {"type": "matchRepository"}
   }
   ```

   One object with `keyPaths` accepts a signature by either key. **Never two objects** (one per
   key): containers/image requires every object in the list, so two objects mean both
   signatures, and an image signed with only one key is refused.
3. Sign the bridge images with the **old** key (the PCs that update to them still trust only
   the old key). From then on, sign every image with both keys (`cosign sign` once with each),
   while PCs move to the bridge. Machines can skip releases, so keep dual-signing for as long as
   an un-updated PC may still come back.
4. Retire the old key: a release whose policy names only the new key, signed with both. Then
   sign with the new key only, and replace the secret.

Proof of the rule in step 2 (a local registry, cosign v3.1.3, Fedora 44's skopeo 1.22.3): a
policy with one `keyPaths` requirement accepted images signed by either key and refused an
unsigned one; two requirement objects refused an image signed by only one of the keys.

## Test builds

`packaging/os/make-test-key.sh` writes a throwaway public key to `target/os/test-key/cosign.pub`
for local builds: set `SIGNING_PUBKEY_FILE` to it (`build-image.sh` and `check-image.sh` read
it). Nothing is signed with it, and an image built with it refuses every published image. Never
publish such an image.
