// The released copy a measurement pins, from the subject `runx.skill.validate`
// read by its registry reference: the reference, and the one digest every
// copy of it carries, published, bundled or served, which a turn's load of
// it binds. Only a released copy has one digest: a source directory's
// package digest isn't its distribution digest, so measuring one is refused.
export function released(subject) {
  const { package_digest: packageDigest, distribution_digest: distributionDigest } = subject.inspect;
  if (!distributionDigest || distributionDigest !== packageDigest) {
    throw new Error(
      `${subject.requested_path} is not a released copy: name the package by its registry reference, <publisher>/<name>@<version>`,
    );
  }
  return { ref: subject.resolved_path, revision: distributionDigest };
}
