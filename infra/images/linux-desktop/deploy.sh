#!/usr/bin/env bash
# Deploy (create or update) the Linux desktop AMI pipeline, and optionally run
# it (TASK-137).
#
# Everything about the image is in this directory: component.yaml is what runs
# on the build instance, stack.yaml wires it into an Image Builder component,
# recipe, infrastructure configuration, distribution configuration and
# pipeline. This script is the only thing that needs AWS credentials.
#
#   infra/images/linux-desktop/deploy.sh                 # deploy only
#   infra/images/linux-desktop/deploy.sh --run           # deploy, then build
#   infra/images/linux-desktop/deploy.sh --run --wait    # ... and wait for the AMI
#
# Image Builder components and recipes are immutable, so the version is
# derived from the content of component.yaml: editing it produces a new
# version automatically and redeploying an unchanged one is a no-op.
#
# Cost: the build runs on one g4dn.xlarge (us-east-1 on-demand 0.526 USD/h)
# for the build phase and a second one for the test phase, about 40 minutes of
# instance time in total, so roughly 0.40 USD per build plus a few cents of
# EBS snapshot. Nothing is left running: TerminateInstanceOnFailure is on.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
stack=${STACK_NAME:-subordinate-linux-desktop}
region=${AWS_REGION:-us-east-1}
release_tag=latest
parent_image=""
component_version=""
run=0
wait_for_image=0

# The RunsOn base image `ubuntu24-gpu-x64` resolves to. Verified against
# RUNS_ON_AMI_ID in a gpu-smoke run rather than guessed.
base_owner=135269210855
base_name='runs-on-v2.2-ubuntu24-gpu-x64-*'

usage() {
    sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options:
  --run                    start a pipeline execution after deploying
  --wait                   with --run, poll until the image is AVAILABLE or FAILED
  --release-tag TAG        release to bake in (default: latest v* release)
  --parent-image AMI       base AMI (default: newest RunsOn ubuntu24-gpu-x64)
  --component-version X.Y.Z  override the content-derived component version
  --stack NAME             CloudFormation stack name
  --region REGION          AWS region
  -h, --help               this text
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
    --run) run=1 ;;
    --wait) wait_for_image=1 ;;
    --release-tag) release_tag=${2:?--release-tag needs a tag}; shift ;;
    --parent-image) parent_image=${2:?--parent-image needs an AMI id}; shift ;;
    --component-version) component_version=${2:?--component-version needs X.Y.Z}; shift ;;
    --stack) stack=${2:?--stack needs a name}; shift ;;
    --region) region=${2:?--region needs a region}; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "deploy.sh: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

die() { echo "deploy.sh: $*" >&2; exit 1; }

command -v aws >/dev/null 2>&1 || die "the AWS CLI is not on PATH"

# --------------------------------------------------------------- base image
if [ -z "$parent_image" ]; then
    parent_image=$(aws ec2 describe-images --region "$region" \
        --executable-users all \
        --filters "Name=name,Values=$base_name" \
        --output json \
        | python3 -c '
import json, sys
images = json.load(sys.stdin)["Images"]
images = [i for i in images if i.get("OwnerId") == "'"$base_owner"'"]
if not images:
    raise SystemExit("no RunsOn base image matched")
print(sorted(images, key=lambda i: i["CreationDate"])[-1]["ImageId"])
')
fi
[ -n "$parent_image" ] || die "could not resolve the RunsOn base image"

# ------------------------------------------------------- component version
# major.minor is a human decision; the patch is the content of component.yaml,
# so an edited component is always a new immutable version.
if [ -z "$component_version" ]; then
    base=$(tr -d '[:space:]' <"$here/VERSION")
    digest=$(sha256sum "$here/component.yaml" | cut -c1-4)
    component_version="$base.$((16#$digest))"
fi

# -------------------------------------------------------------- networking
# There is no default VPC in this account, so the build instance goes in one
# of the RunsOn public subnets (it needs outbound HTTPS for SSM, GitHub and
# crates.io).
read -r subnet_id security_group_id <<EOF
$(aws ec2 describe-subnets --region "$region" --output json | python3 -c '
import json, sys
subnets = [s for s in json.load(sys.stdin)["Subnets"]
           if s["MapPublicIpOnLaunch"]
           and any(t["Key"] == "Name" and t["Value"].startswith("runs-on-Public")
                   for t in s.get("Tags", []))]
if not subnets:
    raise SystemExit("no runs-on public subnet found")
subnets.sort(key=lambda s: s["SubnetId"])
print(subnets[0]["SubnetId"], subnets[0]["VpcId"])
')
EOF
vpc_id=$security_group_id
security_group_id=$(aws ec2 describe-security-groups --region "$region" \
    --output json | python3 -c '
import json, sys
groups = [g for g in json.load(sys.stdin)["SecurityGroups"]
          if g["VpcId"] == "'"$vpc_id"'" and g["GroupName"] == "default"]
if not groups:
    raise SystemExit("no default security group in '"$vpc_id"'")
print(groups[0]["GroupId"])
')

echo "==> stack             $stack ($region)"
echo "==> parent image      $parent_image"
echo "==> component version $component_version"
echo "==> release tag       $release_tag"
echo "==> subnet / sg       $subnet_id / $security_group_id"

# -------------------------------------------------------- component upload
# An inline component body would be simpler, but AWS::ImageBuilder::Component
# caps `Data` at 16000 characters and this component is longer, so it travels
# through S3. The key carries the version, so every version keeps its own
# object and an old image can still be explained.
account=$(aws sts get-caller-identity --output json \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["Account"])')
assets_bucket=${ASSETS_BUCKET:-subordinate-imagebuilder-$account}
if ! aws s3api head-bucket --bucket "$assets_bucket" >/dev/null 2>&1; then
    echo "==> creating s3://$assets_bucket"
    aws s3api create-bucket --bucket "$assets_bucket" --region "$region" >/dev/null
    aws s3api put-public-access-block --bucket "$assets_bucket" \
        --public-access-block-configuration \
        'BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true'
fi
component_key="linux-desktop/component-$component_version.yaml"
aws s3 cp "$here/component.yaml" "s3://$assets_bucket/$component_key" >/dev/null
component_uri="s3://$assets_bucket/$component_key"
echo "==> component         $component_uri"

# ---------------------------------------------------------------- deploy
aws cloudformation deploy \
    --region "$region" \
    --stack-name "$stack" \
    --template-file "$here/stack.yaml" \
    --capabilities CAPABILITY_NAMED_IAM \
    --no-fail-on-empty-changeset \
    --parameter-overrides \
        ComponentUri="$component_uri" \
        LogBucket="$assets_bucket" \
        ParentImage="$parent_image" \
        ComponentVersion="$component_version" \
        ReleaseTag="$release_tag" \
        BuildSubnetId="$subnet_id" \
        BuildSecurityGroupId="$security_group_id"

pipeline_arn=$(aws cloudformation describe-stacks --region "$region" \
    --stack-name "$stack" --output json | python3 -c '
import json, sys
outputs = json.load(sys.stdin)["Stacks"][0]["Outputs"]
print(next(o["OutputValue"] for o in outputs if o["OutputKey"] == "PipelineArn"))
')
echo "==> pipeline          $pipeline_arn"

[ "$run" -eq 1 ] || exit 0

# ------------------------------------------------------------------- build
image_arn=$(aws imagebuilder start-image-pipeline-execution \
    --region "$region" --image-pipeline-arn "$pipeline_arn" \
    --output json | python3 -c 'import json,sys; print(json.load(sys.stdin)["imageBuildVersionArn"])')
echo "==> image build       $image_arn"

[ "$wait_for_image" -eq 1 ] || exit 0

while :; do
    state=$(aws imagebuilder get-image --region "$region" --image-build-version-arn "$image_arn" \
        --output json | python3 -c '
import json, sys
image = json.load(sys.stdin)["image"]
state = image["state"]
print(state["status"], state.get("reason", ""), sep="\t")
')
    status=${state%%$'\t'*}
    echo "$(date -u +%H:%M:%S) $state"
    case "$status" in
    AVAILABLE)
        aws imagebuilder get-image --region "$region" --image-build-version-arn "$image_arn" \
            --output json | python3 -c '
import json, sys
for ami in json.load(sys.stdin)["image"]["outputResources"]["amis"]:
    print("AMI", ami["region"], ami["image"], ami.get("name", ""))
'
        exit 0
        ;;
    FAILED|CANCELLED|DEPRECATED)
        echo "deploy.sh: image build ended $status" >&2
        exit 1
        ;;
    esac
    sleep 60
done
