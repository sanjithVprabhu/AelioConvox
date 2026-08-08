import type { RuntimeArtifactV1 } from './contracts.js';

function mathArtifact(operation: Extract<RuntimeArtifactV1['nodes'][number], { type: 'compute' }>['operation']): RuntimeArtifactV1 {
  const artifactId = `builtin.math.${operation}`;
  return {
    apiVersion: 'aelio.runtime.artifact/v1', artifactId, version: '1.0.0', digest: `builtin:${artifactId}:1.0.0`, status: 'approved', createdAt: 0,
    entryNodeId: 'compute',
    nodes: [
      { id: 'compute', type: 'compute', operation, input: { $ref: 'input' }, next: 'end' },
      { id: 'end', type: 'end', output: { $ref: 'compute' } },
    ],
  };
}

/** The installed catalog is typed data, version-pinned and promotable like tenant artifacts. */
export const DEFAULT_RUNTIME_ARTIFACTS: readonly RuntimeArtifactV1[] = [
  mathArtifact('add'), mathArtifact('subtract'), mathArtifact('multiply'), mathArtifact('divide'),
  mathArtifact('sum'), mathArtifact('count'), mathArtifact('average'),
] as const;

export function findDefaultRuntimeArtifact(artifactId: string): RuntimeArtifactV1 | undefined {
  return DEFAULT_RUNTIME_ARTIFACTS.find((artifact) => artifact.artifactId === artifactId);
}
