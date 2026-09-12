#!/usr/bin/env bash
# Builds (or rebuilds) the Windows NVIDIA desktop runner AMI with EC2 Image
# Builder. TASK-138; see README.md in this directory for what the image is for
# and what it costs.
#
#   ./build.sh 1.0.1            # create version 1.0.1 and start a build
#   ./build.sh 1.0.1 --no-run   # create the resources, do not start a build
#
# SKIP_IAM=1 leaves the two IAM roles and the security group alone, for a
# rebuild in an account where they already exist and the caller would rather
# not hold IAM permissions.
#
# Everything is idempotent except the component and recipe versions, which
# Image Builder makes immutable: pass a new semantic version each time.
set -euo pipefail

VERSION="${1:?usage: build.sh <semantic-version> [--no-run]}"
RUN="${2:-}"

REGION=us-east-1
ACCOUNT=731537225673
NAME=subordinate-windows-desktop
ROLE=SubordinateWindowsDesktopImageBuilder
WORKFLOW_ROLE=SubordinateImageBuilderWorkflow
VPC=vpc-0358c69a2187d56aa
SUBNET=subnet-06dfd78c962937569
# RunsOn's stock Windows image. Refresh this when RunsOn publishes a new one:
#   aws ec2 describe-images --owners 135269210855 \
#     --filters Name=name,Values=runs-on-*-windows22-full-x64-*
PARENT_IMAGE=ami-0142806d2d4aadc50

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
aws() { command aws --region "$REGION" "$@"; }

workflow_role_arn="arn:aws:iam::$ACCOUNT:role/$WORKFLOW_ROLE"
if [ "${SKIP_IAM:-}" = "1" ]; then
echo "== IAM and security group left alone (SKIP_IAM=1)"
sg="$(aws ec2 describe-security-groups \
      --filters "Name=group-name,Values=$NAME" "Name=vpc-id,Values=$VPC" \
      --output text --query 'SecurityGroups[0].GroupId')"
echo "   $sg"
else

echo "== instance profile"
if ! aws iam get-role --role-name "$ROLE" >/dev/null 2>&1; then
  aws iam create-role --role-name "$ROLE" \
    --assume-role-policy-document "file://$here/iam-trust-policy.json" \
    --description "Build instance for the Subordinate Windows desktop AMI (TASK-138)" >/dev/null
fi
aws iam attach-role-policy --role-name "$ROLE" \
  --policy-arn arn:aws:iam::aws:policy/EC2InstanceProfileForImageBuilder
aws iam attach-role-policy --role-name "$ROLE" \
  --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
aws iam put-role-policy --role-name "$ROLE" --policy-name subordinate-windows-desktop \
  --policy-document "file://$here/iam-inline-policy.json"
if ! aws iam get-instance-profile --instance-profile-name "$ROLE" >/dev/null 2>&1; then
  aws iam create-instance-profile --instance-profile-name "$ROLE" >/dev/null
  aws iam add-role-to-instance-profile --instance-profile-name "$ROLE" --role-name "$ROLE" >/dev/null
  echo "   waiting for the instance profile to propagate"
  sleep 20
fi
echo "   $ROLE ready"
echo "   NOTE: the test-media bucket policy must also allow this role - see README.md"

echo "== workflow execution role"
# A custom build workflow (this image is captured without Sysprep) needs a role
# Image Builder assumes to drive the build; the managed service-linked role
# only covers its own workflows.
if ! aws iam get-role --role-name "$WORKFLOW_ROLE" >/dev/null 2>&1; then
  aws iam create-role --role-name "$WORKFLOW_ROLE" \
    --assume-role-policy-document "file://$here/iam-workflow-trust-policy.json" \
    --description "Image Builder custom workflow execution role (TASK-138)" >/dev/null
  sleep 10
fi
aws iam put-role-policy --role-name "$WORKFLOW_ROLE" --policy-name subordinate-windows-desktop \
  --policy-document "file://$here/iam-workflow-policy.json"
workflow_role_arn="arn:aws:iam::$ACCOUNT:role/$WORKFLOW_ROLE"
echo "   $workflow_role_arn"

echo "== security group"
sg="$(aws ec2 describe-security-groups \
      --filters "Name=group-name,Values=$NAME" "Name=vpc-id,Values=$VPC" \
      --output text --query 'SecurityGroups[0].GroupId' 2>/dev/null || true)"
if [ -z "$sg" ] || [ "$sg" = "None" ]; then
  sg="$(aws ec2 create-security-group --group-name "$NAME" --vpc-id "$VPC" \
        --description "Egress only, for the Subordinate Windows desktop AMI build (TASK-138)" \
        --output text --query 'GroupId')"
fi
echo "   $sg"
fi

# Four components rather than one: CreateComponent caps a component document
# at 16000 characters, and the whole recipe is about twice that.
echo "== components $VERSION"
n=0
sed_args=(-e "s|SEMANTIC_VERSION|$VERSION|" -e "s|PARENT_IMAGE|$PARENT_IMAGE|")
for part in gpu session helper payload; do
  n=$((n + 1))
  arn="$(aws imagebuilder create-component \
    --name "$NAME-$part" --semantic-version "$VERSION" --platform Windows \
    --data "file://$here/component-$n-$part.yml" \
    --output text --query 'componentBuildVersionArn')"
  echo "   $arn"
  sed_args+=(-e "s|COMPONENT_ARN_$n|$arn|")
done

echo "== build workflow $VERSION"
workflow="$(aws imagebuilder create-workflow --name "$NAME-no-sysprep" \
  --semantic-version "$VERSION" --type BUILD \
  --data "file://$here/workflow-build-no-sysprep.yml" \
  --output text --query 'workflowBuildVersionArn')"
echo "   $workflow"

echo "== recipe $VERSION"
sed "${sed_args[@]}" "$here/recipe.json" > "$tmp/recipe.json"
recipe="$(aws imagebuilder create-image-recipe --cli-input-json "file://$tmp/recipe.json" \
  --output text --query 'imageRecipeArn')"
echo "   $recipe"

echo "== infrastructure configuration"
sed -e "s|SECURITY_GROUP_ID|$sg|" "$here/infrastructure-config.json" > "$tmp/infra.json"
infra="arn:aws:imagebuilder:$REGION:$ACCOUNT:infrastructure-configuration/$NAME"
if aws imagebuilder get-infrastructure-configuration --infrastructure-configuration-arn "$infra" >/dev/null 2>&1; then
  # An update takes the same document without its name, which is immutable.
  grep -v '"name"' "$tmp/infra.json" > "$tmp/infra-update.json"
  aws imagebuilder update-infrastructure-configuration --cli-input-json "file://$tmp/infra-update.json" \
    --infrastructure-configuration-arn "$infra" >/dev/null
else
  infra="$(aws imagebuilder create-infrastructure-configuration --cli-input-json "file://$tmp/infra.json" \
    --output text --query 'infrastructureConfigurationArn')"
fi
echo "   $infra"

echo "== distribution configuration"
dist="arn:aws:imagebuilder:$REGION:$ACCOUNT:distribution-configuration/$NAME"
if aws imagebuilder get-distribution-configuration --distribution-configuration-arn "$dist" >/dev/null 2>&1; then
  grep -v '"name": "subordinate-windows-desktop"' "$here/distribution-config.json" > "$tmp/dist-update.json"
  aws imagebuilder update-distribution-configuration --cli-input-json "file://$tmp/dist-update.json" \
    --distribution-configuration-arn "$dist" >/dev/null
else
  dist="$(aws imagebuilder create-distribution-configuration --cli-input-json "file://$here/distribution-config.json" \
    --output text --query 'distributionConfigurationArn')"
fi
echo "   $dist"

echo "== pipeline"
pipeline="arn:aws:imagebuilder:$REGION:$ACCOUNT:image-pipeline/$NAME"
if aws imagebuilder get-image-pipeline --image-pipeline-arn "$pipeline" >/dev/null 2>&1; then
  aws imagebuilder update-image-pipeline --image-pipeline-arn "$pipeline" \
    --image-recipe-arn "$recipe" --infrastructure-configuration-arn "$infra" \
    --distribution-configuration-arn "$dist" --status ENABLED \
    --image-tests-configuration imageTestsEnabled=false \
    --execution-role "$workflow_role_arn" \
    --workflows "workflowArn=$workflow" >/dev/null
else
  pipeline="$(aws imagebuilder create-image-pipeline --name "$NAME" \
    --description "Windows NVIDIA desktop runner AMI (TASK-138). Rebuild at least every 30 days: GitHub stops dispatching jobs to a runner agent older than that." \
    --image-recipe-arn "$recipe" --infrastructure-configuration-arn "$infra" \
    --distribution-configuration-arn "$dist" --status ENABLED \
    --image-tests-configuration imageTestsEnabled=false \
    --execution-role "$workflow_role_arn" \
    --workflows "workflowArn=$workflow" \
    --output text --query 'imagePipelineArn')"
fi
echo "   $pipeline"

if [ "$RUN" = "--no-run" ]; then
  echo "== not started (--no-run)"
  exit 0
fi

echo "== starting a build"
image="$(aws imagebuilder start-image-pipeline-execution --image-pipeline-arn "$pipeline" \
  --output text --query 'imageBuildVersionArn')"
echo "   $image"
echo
echo "Watch it with:"
echo "  aws imagebuilder get-image --region $REGION --image-build-version-arn $image \\"
echo "    --output text --query 'image.state.status'"
echo "Build logs land in s3://subordinate-test-media-731537225673/imagebuilder-logs/."
