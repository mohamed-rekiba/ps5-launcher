# PS5 Launcher OS installer: Fedora's network installer with this file (packaging/os/build-iso.sh
# writes the image name in place of @IMAGE@, and the digest in place of @MAIN_DIGEST@).
#
# The installer asks only for the disk, the user and the time zone. The OS itself is downloaded
# from the container registry, so the PC needs an internet connection.
#
keyboard --vckeymap=us --xlayouts=us
rootpw --lock

# The image comes by digest: the exact main image that CI checked and signed, which
# build-iso.sh writes in place of @MAIN_DIGEST@. The installer's bootc does
# not check signatures (Anaconda runs `bootc install to-filesystem --source-imgref ...
# --target-imgref ...` under the installer's own containers policy), and a digest cannot be
# swapped. The installed system then follows the tag; at its first start
# ps5-signature-policy.service makes bootc enforce the image's signature policy from then on.
%pre --erroronfail --log=/tmp/ps5-launcher-os-pre.log
image=@IMAGE@
tag=main
digest=@MAIN_DIGEST@
echo "Installing $image@$digest, following $image:$tag"
# The source needs a transport prefix; the update target must not have one.
echo "bootc --source-imgref=registry:$image@$digest --target-imgref=$image:$tag" > /tmp/ps5-launcher-os-source.ks
%end
%include /tmp/ps5-launcher-os-source.ks
