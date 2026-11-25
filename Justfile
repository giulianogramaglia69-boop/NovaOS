# NovaOS Justfile
#
# Build one image per desktop from a single Containerfile. The desktop is
# encoded in the image name: `just build novaos-niri 45` builds the Niri
# image (DE=niri, base base-main); `just build novaos-gnome 45` builds the
# GNOME image (DE=gnome, base silverblue-main).
#
#   just build [image] [tag]   build a container image (default novaos-niri:45)
#   just build-qcow2           container image -> bootable qcow2 (via BIB)
#   just build-iso             container image -> installable ISO (via BIB)
#   just run-vm-qcow2          boot the qcow2 in QEMU
#
# Adapted from the projectbluefin/finpilot Justfile (build, tag-images,
# _build-bib and run-vm recipes), slimmed to what NovaOS needs.

export FEDORA_MAJOR_VERSION := env("FEDORA_MAJOR_VERSION", "45")
export IMAGE_VENDOR := env("IMAGE_VENDOR", env("GITHUB_REPOSITORY_OWNER", "giulianogramaglia69-boop"))
export PODMAN := env("PODMAN", "podman")
export bib_image := env("BIB_IMAGE", "ghcr.io/osbuild/bootc-image-builder:latest@sha256:b2f639986cb1cad3dae1de76ded19c081ce4699b053115296c7460919d2edf47")
export vm_ram := env("VM_RAM", "8192")
export vm_cpus := env("VM_CPUS", "4")

[private]
default:
    @just --list

# Check Just Syntax
check:
    just --unstable --fmt --check -f Justfile

# Fix Just Syntax
fix:
    just --unstable --fmt -f Justfile

# Runs shellcheck on the build scripts
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v shellcheck &> /dev/null; then
        echo "shellcheck could not be found."
        exit 1
    fi
    shellcheck build/*.sh

# Clean build artifacts
clean:
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf output/ sbom-current/
    find . -maxdepth 1 -name '_build*' -exec rm -rf {} +

# Build the container image with Podman.
#
# Arguments:
#   $target_image - the image to build (default: novaos-niri)
#   $tag          - the image tag (default: 45)
#
# The desktop (DE) and base image are derived from the target image name:
# novaos-gnome -> DE=gnome, base silverblue-main; anything else ->

# DE=<suffix>, base base-main. Version is <tag>.<date>, e.g. 45.20261125.
build $target_image="novaos-niri" $tag="45":
    #!/usr/bin/env bash
    set -euo pipefail

    image_name="${target_image#localhost/}"
    de="${image_name#novaos-}"
    case "${de}" in
        gnome|cosmic|hyprland|mangowc|niri) ;;
        *)
            echo "ERROR: unknown desktop in image name '${image_name}' (expected novaos-<gnome|cosmic|hyprland|mangowc|niri>)"
            exit 1
            ;;
    esac
    if [[ "${de}" == "gnome" ]]; then
        base_image_name="silverblue-main"
    else
        base_image_name="base-main"
    fi

    ver="${tag}.$(date +%Y%m%d)"
    if command -v skopeo &>/dev/null; then
        repotags=$(mktemp -t repotags.XXXXXXXX.json) || { echo "ERROR: mktemp failed"; exit 1; }
        trap 'rm -f "${repotags}"' EXIT
        skopeo list-tags "docker://ghcr.io/${IMAGE_VENDOR}/${image_name}" >"${repotags}" 2>/dev/null \
            || echo '{"Tags":[]}' >"${repotags}"
        if [[ $(jq "any(.Tags[]; contains(\"${ver}\"))" "${repotags}") == "true" ]]; then
            POINT=1
            while [[ $(jq "any(.Tags[]; contains(\"${ver}.${POINT}\"))" "${repotags}") == "true" ]]; do
                ((POINT++))
            done
            ver="${ver}.${POINT}"
            echo "Tag collision detected; using version ${ver}"
        fi
    fi

    BUILD_ARGS=()
    BUILD_ARGS+=("--build-arg" "VERSION=${ver}")
    BUILD_ARGS+=("--build-arg" "IMAGE_NAME=${image_name}")
    BUILD_ARGS+=("--build-arg" "IMAGE_VENDOR=${IMAGE_VENDOR}")
    BUILD_ARGS+=("--build-arg" "UBLUE_IMAGE_TAG=${tag}")
    BUILD_ARGS+=("--build-arg" "BASE_IMAGE_NAME=${base_image_name}")
    BUILD_ARGS+=("--build-arg" "FEDORA_MAJOR_VERSION=${FEDORA_MAJOR_VERSION}")
    BUILD_ARGS+=("--build-arg" "DE=${de}")
    BUILD_ARGS+=("--build-arg" "IMAGE_CREATED=$(date -u +%Y\-%m\-%d\T%H\:%M\:%S\Z)")
    if [[ -z "$(git status -s 2>/dev/null)" && -d .git ]]; then
        BUILD_ARGS+=("--build-arg" "SHA_HEAD_SHORT=$(git rev-parse --short HEAD)")
    fi

    ${PODMAN} build \
        "${BUILD_ARGS[@]}" \
        --pull=newer \
        --tag "${target_image}:${tag}" \
        .

# Tag an image with extra tags (Bluefin pattern: separate tagging from pushing)
tag-images $image_name="" $default_tag="" $tags="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -z "${image_name}" || -z "${default_tag}" || -z "${tags}" ]]; then
        echo "Usage: just tag-images <image_name> <default_tag> <tags>"
        exit 1
    fi
    IMAGE=$(${PODMAN} inspect "localhost/${image_name}:${default_tag}" | jq -r '.[].Id')
    ${PODMAN} untag "localhost/${image_name}:${default_tag}"
    for tag in ${tags}; do
        ${PODMAN} tag "${IMAGE}" "${image_name}:${tag}"
    done
    ${PODMAN} tag "${IMAGE}" "${image_name}:${default_tag}"
    echo "Tagged ${image_name} with: ${tags}"

# Make the locally built image visible to rootful podman so Bootc Image Builder

# can read it. No-op when already root.
_rootful_load_image $target_image="novaos-niri" $tag="45":
    #!/usr/bin/env bash
    set -eoux pipefail
    if [[ -n "${SUDO_USER:-}" || "${UID}" -eq "0" ]]; then
        echo "Already root or running under sudo, no need to load image from user podman."
        exit 0
    fi
    if ! podman inspect -t image "${target_image}:${tag}" >/dev/null 2>&1; then
        sudo podman pull "${target_image}:${tag}"
        exit 0
    fi
    USER_IMG_ID=$(podman images -q --filter reference="${target_image}:${tag}")
    ID=$(sudo podman images -q --filter reference="${target_image}:${tag}")
    if [[ "${ID}" != "${USER_IMG_ID}" ]]; then
        COPYTMP=$(mktemp -p "${PWD}" -d -t _build_podman_scp.XXXXXXXXXX)
        sudo TMPDIR=${COPYTMP} podman image scp ${UID}@localhost::"${target_image}:${tag}" root@localhost::"${target_image}:${tag}"
        rm -rf "${COPYTMP}"
    fi

# Convert a container image into a bootable disk with Bootc Image Builder.
# type is qcow2, raw or iso; config is the BIB config file to use

# (iso/disk.toml for qcow2 and raw, iso/iso.toml for iso).
_build-bib $target_image $tag $type $config: (_rootful_load_image target_image tag)
    #!/usr/bin/env bash
    set -euo pipefail
    args="--type ${type} "
    args+="--use-librepo=True "
    args+="--rootfs=btrfs"
    build_image="${target_image}:${tag}"
    if [[ "${type}" == "iso" ]]; then
        image_info=$(sudo podman run --rm --entrypoint /usr/bin/cat \
            "${target_image}:${tag}" /usr/share/ublue-os/image-info.json)
        image_ref=$(jq -r '."image-ref"' <<<"${image_info}" | sed 's|.*docker://||')
        build_image="${image_ref}:${tag}"
        sudo podman tag "${target_image}:${tag}" "${build_image}"
    fi
    BUILDTMP=$(mktemp -p "${PWD}" -d -t _build-bib.XXXXXXXXXX)
    trap 'sudo rm -rf "${BUILDTMP}"' EXIT
    sudo podman run \
      --rm \
      --privileged \
      --net=host \
      --security-opt label=type=unconfined_t \
      -v "${PWD}/${config}:/config.toml:ro" \
      -v "${BUILDTMP}:/output" \
      -v /var/lib/containers/storage:/var/lib/containers/storage \
      "${bib_image}" \
      ${args} \
      "${build_image}"
    mkdir -p output
    for artifact in "${BUILDTMP}"/*; do
        sudo rm -rf "output/$(basename "${artifact}")"
    done
    sudo mv -f "${BUILDTMP}"/* output/
    sudo rmdir "${BUILDTMP}"
    sudo chown -R "$(id -u):$(id -g)" output/

# Rebuild the container image first, then convert it (see _build-bib).
_rebuild-bib $target_image $tag $type $config: (build target_image tag) && (_build-bib target_image tag type config)

# Build a QCOW2 virtual machine image
build-qcow2 $target_image=("localhost/novaos-niri") $tag="45": && (_build-bib target_image tag "qcow2" "iso/disk.toml")

# Build a RAW virtual machine image
build-raw $target_image=("localhost/novaos-niri") $tag="45": && (_build-bib target_image tag "raw" "iso/disk.toml")

# Build an ISO install image
build-iso $target_image=("localhost/novaos-niri") $tag="45": && (_build-bib target_image tag "iso" "iso/iso.toml")

# Rebuild a QCOW2 virtual machine image
rebuild-qcow2 $target_image=("localhost/novaos-niri") $tag="45": && (_rebuild-bib target_image tag "qcow2" "iso/disk.toml")

# Rebuild an ISO install image
rebuild-iso $target_image=("localhost/novaos-niri") $tag="45": && (_rebuild-bib target_image tag "iso" "iso/iso.toml")

# The artifact Bootc Image Builder writes for a given --type.
[private]
vm-artifact $type:
    #!/usr/bin/env bash
    set -euo pipefail
    case "${type}" in
        qcow2) echo "output/qcow2/disk.qcow2" ;;
        raw) echo "output/image/disk.raw" ;;
        iso) echo "output/bootiso/install.iso" ;;
        *)
            echo "ERROR: unknown image type '${type}' (expected qcow2, raw or iso)" >&2
            exit 1
            ;;
    esac

# Run a built artifact in a virtual machine (QEMU/KVM).
_run-vm $target_image $tag $type:
    #!/usr/bin/env bash
    set -euo pipefail
    artifact=$(just vm-artifact "${type}")
    if [[ ! -f "${artifact}" ]]; then
        echo "ERROR: ${artifact} not found — run: just build-${type}" >&2
        exit 1
    fi
    if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
        echo "ERROR: qemu-system-x86_64 not found — install qemu-system-x86" >&2
        exit 1
    fi
    ovmf_code=""
    for f in \
        /usr/share/edk2/ovmf/OVMF_CODE.fd \
        /usr/share/OVMF/OVMF_CODE.fd \
        /usr/share/OVMF/OVMF_CODE_4M.fd \
        /usr/share/edk2/x64/OVMF_CODE.4m.fd \
        /usr/share/qemu/OVMF_CODE.fd; do
        [[ -f "${f}" ]] && { ovmf_code="${f}"; break; }
    done
    if [[ -z "${ovmf_code}" ]]; then
        echo "ERROR: OVMF firmware not found — install edk2-ovmf (Fedora) or ovmf (Debian/Ubuntu)" >&2
        exit 1
    fi
    ovmf_vars_src=""
    for f in \
        /usr/share/edk2/ovmf/OVMF_VARS.fd \
        /usr/share/OVMF/OVMF_VARS.fd \
        /usr/share/OVMF/OVMF_VARS_4M.fd \
        /usr/share/edk2/x64/OVMF_VARS.4m.fd \
        /usr/share/qemu/OVMF_VARS.fd; do
        [[ -f "${f}" ]] && { ovmf_vars_src="${f}"; break; }
    done
    ovmf_vars=$(mktemp /tmp/OVMF_VARS.XXXXXX.fd)
    [[ -n "${ovmf_vars_src}" ]] && cp "${ovmf_vars_src}" "${ovmf_vars}"
    trap 'rm -f "${ovmf_vars}"' EXIT
    ssh_port=2222
    while ss -tunalp 2>/dev/null | grep -q ":${ssh_port} "; do
        ssh_port=$(( ssh_port + 1 ))
    done
    args=(
        -machine q35
        -accel kvm
        -cpu host
        -smp "${vm_cpus}"
        -m "${vm_ram}"
        -device virtio-vga
        -device virtio-keyboard
        -device virtio-mouse
        -device virtio-net-pci,netdev=net0
        -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:${ssh_port}-:22"
        -drive "if=pflash,format=raw,readonly=on,file=${ovmf_code}"
        -drive "if=pflash,format=raw,file=${ovmf_vars}"
    )
    if [[ "${type}" == iso ]]; then
        target="output/iso/target.qcow2"
        if [[ ! -f "${target}" ]]; then
            echo "==> Creating the ISO's scratch target disk: ${target} (64G sparse)"
            mkdir -p output/iso
            qemu-img create -f qcow2 "${target}" 64G >/dev/null
        fi
        args+=(-drive "file=${artifact},media=cdrom,readonly=on,format=raw")
        args+=(-drive "file=$(realpath "${target}"),if=virtio,format=qcow2")
        args+=(-boot order=d)
    else
        args+=(-drive "file=${artifact},if=virtio,format=${type}")
        args+=(-boot order=c)
    fi
    if [[ -n "${DISPLAY:-}" || -n "${WAYLAND_DISPLAY:-}" ]]; then
        args+=(-display gtk)
    else
        echo "==> No display detected: serial console on stdio (Ctrl-A X to quit)"
        args+=(-display none -serial mon:stdio)
    fi
    echo "==> Booting ${artifact}"
    echo "    RAM: ${vm_ram}M, CPUs: ${vm_cpus}, ssh: ssh -p ${ssh_port} <user>@127.0.0.1"
    qemu-system-x86_64 "${args[@]}"

# Run a virtual machine from a QCOW2 image
run-vm-qcow2 $target_image=("localhost/novaos-niri") $tag="45": && (_run-vm target_image tag "qcow2")

# Run a virtual machine from an ISO
run-vm-iso $target_image=("localhost/novaos-niri") $tag="45": && (_run-vm target_image tag "iso")
