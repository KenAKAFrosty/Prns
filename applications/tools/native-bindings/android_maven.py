"""Exact app Gradle runtime closure; coordinates are reviewed, artifact hashes observed."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re

BASELINE = Path(__file__).with_name('android-maven-baseline.json')
CONFIGURATIONS = ('debugRuntimeClasspath', 'releaseRuntimeClasspath')
TASK = ':app:prnsRuntimeDependencyInventory'


def gradle_init() -> str:
    # Resolve metadata/artifacts only: this task does not depend on assemble or sign.
    return '''
allprojects { project ->
  if (project.path == ':app') {
    project.plugins.withId('com.android.application') {
      project.tasks.register('prnsRuntimeDependencyInventory') {
        doLast {
          def configurations = [:]
          def projects = [:]
          def artifacts = [:]
          ['debugRuntimeClasspath', 'releaseRuntimeClasspath'].each { name ->
            def configuration = project.configurations.getByName(name)
            def result = configuration.incoming.resolutionResult
            def unresolved = result.allDependencies.findAll {
              it instanceof org.gradle.api.artifacts.result.UnresolvedDependencyResult
            }
            if (!unresolved.empty) {
              throw new GradleException("Unresolved runtime dependencies in ${name}: ${unresolved*.attempted}")
            }
            configurations[name] = result.allComponents.findAll {
              it.id instanceof org.gradle.api.artifacts.component.ModuleComponentIdentifier
            }.collect { "${it.id.group}:${it.id.module}:${it.id.version}".toString() }.unique().sort()
            projects[name] = result.allComponents.findAll {
              it.id instanceof org.gradle.api.artifacts.component.ProjectComponentIdentifier
            }.collect { it.id.displayName }.unique().sort()
            // Filtering project components avoids requesting unbuilt project outputs.
            def external = configuration.incoming.artifactView {
              componentFilter { id -> id instanceof org.gradle.api.artifacts.component.ModuleComponentIdentifier }
            }.artifacts
            if (!external.failures.empty) {
              throw new GradleException("Unresolved runtime artifacts in ${name}: ${external.failures}")
            }
            artifacts[name] = external.artifacts.collect { artifact ->
              def id = artifact.id.componentIdentifier
              def digest = java.security.MessageDigest.getInstance('SHA-256')
              artifact.file.withInputStream { stream ->
                byte[] buffer = new byte[65536]
                int count
                while ((count = stream.read(buffer)) != -1) { digest.update(buffer, 0, count) }
              }
              [coordinate: "${id.group}:${id.module}:${id.version}".toString(),
               file_name: artifact.file.name, sha256: digest.digest().encodeHex().toString()]
            }.unique().sort { a, b -> (a.coordinate <=> b.coordinate) ?: (a.file_name <=> b.file_name) }
          }
          def output = System.getenv('PRNS_ANDROID_MAVEN_REPORT')
          if (!output) { throw new GradleException('PRNS_ANDROID_MAVEN_REPORT is required') }
          new File(output).text = groovy.json.JsonOutput.prettyPrint(groovy.json.JsonOutput.toJson(
            [schema_version: 1, configurations: configurations, project_components: projects, artifacts: artifacts])) + '\\n'
        }
      }
    }
  }
}
'''


def coordinates(document: dict) -> dict[str, list[str]]:
    if document.get('schema_version') != 1:
        raise ValueError('Unsupported Android Maven inventory schema')
    values = document.get('configurations')
    if not isinstance(values, dict) or set(values) != set(CONFIGURATIONS):
        raise ValueError('Both exact debugRuntimeClasspath and releaseRuntimeClasspath are required')
    for name, items in values.items():
        if not isinstance(items, list) or not items or not all(isinstance(item, str) and re.fullmatch(r'[^:\s]+:[^:\s]+:[^:\s]+', item) for item in items):
            raise ValueError(f'Invalid or empty Maven coordinate closure: {name}')
        if items != sorted(set(items)):
            raise ValueError(f'Maven coordinate closure must be sorted and unique: {name}')
    return values


def compare(actual: dict, baseline: dict) -> None:
    observed, expected = coordinates(actual), coordinates(baseline)
    differences = []
    for name in CONFIGURATIONS:
        if observed[name] != expected[name]:
            differences.append(f'{name}: added={sorted(set(observed[name]) - set(expected[name]))}, removed={sorted(set(expected[name]) - set(observed[name]))}')
    if differences:
        raise ValueError('Android Maven runtime closure drifted; review baseline and notices before acceptance. ' + '; '.join(differences))


def check_report(raw_path: Path, output: Path, baseline_path: Path = BASELINE) -> dict:
    raw = raw_path.read_bytes()
    actual = json.loads(raw)
    baseline_bytes = baseline_path.read_bytes()
    baseline = json.loads(baseline_bytes)
    compare(actual, baseline)
    for key in ('artifacts', 'project_components'):
        if not isinstance(actual.get(key), dict) or set(actual[key]) != set(CONFIGURATIONS):
            raise ValueError(f'Missing per-configuration {key}')
    for name in CONFIGURATIONS:
        projects = actual['project_components'][name]
        if not isinstance(projects, list) or not all(isinstance(item, str) for item in projects):
            raise ValueError(f'Invalid project component inventory: {name}')
        artifacts = actual['artifacts'][name]
        if not isinstance(artifacts, list) or not artifacts:
            raise ValueError(f'Missing external artifact evidence: {name}')
        seen = set()
        for artifact in artifacts:
            if not isinstance(artifact, dict) or artifact.get('coordinate') not in actual['configurations'][name]:
                raise ValueError(f'Artifact outside resolved closure: {name}')
            file_name = artifact.get('file_name')
            if not isinstance(file_name, str) or Path(file_name).name != file_name or not file_name:
                raise ValueError('Artifact evidence must use a filename, not a machine-local cache path')
            if not isinstance(artifact.get('sha256'), str) or not re.fullmatch(r'[0-9a-f]{64}', artifact['sha256']):
                raise ValueError('Missing exact external runtime artifact hash')
            key = artifact['coordinate'], file_name
            if key in seen:
                raise ValueError(f'Duplicate artifact evidence: {key}')
            seen.add(key)
    report = dict(actual,
        baseline=baseline_path.name,
        baseline_sha256=hashlib.sha256(baseline_bytes).hexdigest(),
        inventory_sha256=hashlib.sha256(raw).hexdigest(),
        union=sorted(set().union(*actual['configurations'].values())),
        qualification='Resolved debug and release runtime metadata/artifacts only. Release resolution is not compiled, signed, device or release acceptance. Coordinates alone do not bind artifact bytes; observed external input hashes are included. Project source dependencies and repository provenance require their owning source inventories.')
    output.write_text(json.dumps(report, indent=2) + '\n')
    return report
