#!/bin/bash
arg="$1"
set -eu

function test_command {
    local cmd=${*:1}
    set +e
    if ! $cmd &> /dev/null; then
        echo "error: command $cmd not found"
        exit 1
    fi
    set -e
}

function test_commands {
    test_command curl --version
    test_command mktemp --help
}

function print_help {
    >&2 echo "usage:"
    >&2 echo "    delete-drive.sh DRIVE_NAME"
    >&2 echo "    delete-drive.sh --all"
}

function api {
    local call_name=$1
    local query=$2
    local stdout=$(mktemp)
    local stderr=$(mktemp)
    if ! curl "https://${OSC_ENDPOINT_API}/${call_name}" -H 'Content-Type: application/json' -d "$query" --user $OSC_ACCESS_KEY:$OSC_SECRET_KEY --aws-sigv4 "osc" > "$stdout" 2> "$stderr"; then
        >&2 echo "error while performing API call $call_name"
        >&2 echo "query: $query"
        >&2 echo "stdout: $(cat "$stdout")"
        >&2 echo "stderr: $(cat "$stderr")"
    fi
    cat "$stdout"
    rm "$stdout"
    rm "$stderr"
}

function list_bsu_of_drive {
    local drive_name=$1
    api ReadVolumes "{\"Filters\": {\"TagKeys\":[\"osc.bsud.drive-name\"], \"TagValues\":[\"${drive_name}\"]}}" | jq .Volumes[].VolumeId | xargs
}

function list_all_drives {
    api ReadVolumes "{\"Filters\": {\"TagKeys\":[\"osc.bsud.drive-name\"]}}" | jq .Volumes[].Tags[].Value | xargs
}

function detach_and_delete_bsu {
    local bsu_id=$1
    detach_bsu "$bsu_id" || true
    sleep 10
    delete_bsu "$bsu_id" || true
}

function detach_bsu {
    local bsu_id=$1
    echo "detaching BSU $bsu_id..."
    api UnlinkVolume "{\"VolumeId\": \"$bsu_id\"}" > /dev/null || true
}

function delete_bsu {
    local bsu_id=$1
    echo "deleting BSU $bsu_id..."
    api DeleteVolume "{\"VolumeId\": \"$bsu_id\"}" > /dev/null || true
}

function delete_drive {
    local drive_name=$1
    echo "# deleting BSUd drive $drive_name..."

    local lv_path="/dev/${drive_name}/bsud"
    if [ -e $lv_path ]; then
        echo "umounting $lv_path... "
        sudo umount "$lv_path" || true
        echo "disable lv $lv_path... "
        sudo lvchange --activate n "$lv_path" || true
        # In case of forced detach
        echo "dmsetup remove $lv_path... "
        sudo dmsetup remove /dev/${drive_name}/bsud || true
    else
        echo "$lv_path seems not to exist"
    fi

    echo "listing BSU of $drive_name drive..."
    local bsus=$(list_bsu_of_drive "$drive_name")
    if [ -z "$bsus" ]; then
        echo "no BSU to delete"
        exit 0
    fi
    for bsu_id in $bsus; do
        detach_and_delete_bsu "$bsu_id" &
    done
    wait
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)

success=true
if [ -z "${OSC_ACCESS_KEY:-}" ]; then
    >&2 echo 'error: OSC_ACCESS_KEY not set'
    success=false
fi
if [ -z "${OSC_SECRET_KEY:-}" ]; then
    >&2 echo 'error: OSC_SECRET_KEY not set'
    success=false
fi
if [ -z "${OSC_ENDPOINT_API=:-}" ]; then
    >&2 echo 'error: OSC_ENDPOINT_API not set'
    >&2 echo 'please set under the form api.$OSC_REGION.outscale.com/api/v1'
    success=false
fi

if ! $success; then
    exit 1
fi

test_commands
if [ -z "${arg=:-}" ]; then
    print_help
    echo -n "existing BSU drives: "
    list_all_drives
    exit 1
fi

if [ "$arg" = "--all" ]; then
    all_drives="$(list_all_drives)"
    if [ -z "${all_drives}" ]; then
        >&2 echo 'no drive found to be delete'
        exit 0
    fi
    for drive_name in ${all_drives}; do
        delete_drive $drive_name
    done
else 
    delete_drive $drive_name
fi
