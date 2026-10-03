/// Project the whole Graphify graph into Obsidian-native linked notes, a file
/// index, and a Canvas. Graphify remains the source of truth; these files are a
/// navigable view rather than a second hand-maintained graph.
///
/// The projection is complete by default: every community that carries a node
/// becomes a note, and every node appears in exactly one of them. A partial
/// projection is worse than none, because an agent that finds no note for a
/// subsystem cannot tell "absent from the graph" from "below the cutoff" and
/// falls back to reading source — the cost this projection exists to avoid.
/// `limit` still trims the set for tests and for spot projections.

import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/// The Canvas is a spatial view, not an index. Past roughly fifty cards it is
/// slower to read than the Knowledge Map, so it stays capped while the notes do
/// not.
const CANVAS_COMMUNITY_LIMIT = 48
const CANVAS_EDGE_LIMIT = 48
const RELATED_LIMIT = 12

function safeName(value) {
  return String(value ?? 'Unlabelled')
    .replace(/[\\/:*?"<>|#^[\]]/g, '-')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 80) || 'Unlabelled'
}

function quoted(value) {
  return JSON.stringify(String(value))
}

function communityFile(community) {
  return `C${String(community.id).padStart(3, '0')} - ${safeName(community.name)}.md`
}

function noteLink(community) {
  return `[[docs/knowledge/communities/${communityFile(community).slice(0, -3)}|${community.name}]]`
}

function location(node) {
  return `${node.source_file || 'graph'}${node.source_location ? `:${node.source_location}` : ''}`
}

export function generateKnowledge({ graphPath, outputDir, limit = Infinity }) {
  const graph = JSON.parse(readFileSync(graphPath, 'utf8'))
  const nodeById = new Map(graph.nodes.map(node => [node.id, node]))
  const grouped = new Map()
  for (const node of graph.nodes) {
    if (node.community === undefined || node.community === null) continue
    const group = grouped.get(node.community) ?? {
      id: node.community,
      name: node.community_name ?? `Community ${node.community}`,
      nodes: [],
      internalEdges: 0,
      degree: new Map(),
      related: new Map(),
    }
    group.nodes.push(node)
    grouped.set(node.community, group)
  }

  for (const edge of graph.links ?? graph.edges ?? []) {
    const source = nodeById.get(edge.source)
    const target = nodeById.get(edge.target)
    if (!source || !target) continue
    const sourceGroup = grouped.get(source.community)
    const targetGroup = grouped.get(target.community)
    if (!sourceGroup || !targetGroup) continue
    sourceGroup.degree.set(source.id, (sourceGroup.degree.get(source.id) ?? 0) + 1)
    targetGroup.degree.set(target.id, (targetGroup.degree.get(target.id) ?? 0) + 1)
    if (source.community === target.community) {
      sourceGroup.internalEdges += 1
    } else {
      sourceGroup.related.set(target.community, (sourceGroup.related.get(target.community) ?? 0) + 1)
      targetGroup.related.set(source.community, (targetGroup.related.get(source.community) ?? 0) + 1)
    }
  }

  const communities = [...grouped.values()]
    .sort((a, b) => b.nodes.length - a.nodes.length || String(a.id).localeCompare(String(b.id)))
    .slice(0, limit)

  // Clustering routinely leaves single-node communities, and a one-node
  // community takes its node's name -- so several end up called `Default` or
  // `Self`. The notes are distinct files because the filename carries the id,
  // but an index offering four identical link texts cannot be navigated. Only
  // the repeats are qualified; a unique name is left alone.
  const nameCounts = new Map()
  for (const community of communities) {
    nameCounts.set(community.name, (nameCounts.get(community.name) ?? 0) + 1)
  }
  for (const community of communities) {
    if (nameCounts.get(community.name) > 1) {
      community.name = `${community.name} (C${String(community.id).padStart(3, '0')})`
    }
  }
  const selected = new Map(communities.map(community => [community.id, community]))
  mkdirSync(join(outputDir, 'communities'), { recursive: true })

  const indexLines = [
    '---',
    'graphify-generated: true',
    'tags:',
    '  - graphify/map',
    '---',
    '',
    '# Knowledge Map',
    '',
    '> Generated from `graphify-out/graph.json` by `scripts/graph.sh obsidian`. Do not edit generated community notes by hand.',
    '',
    'Every community in the graph has a note below, and every node lives in exactly',
    'one of them. Open [[docs/knowledge/Files|Files]] to go from a source path to its',
    'communities, or [[docs/knowledge/Architecture.canvas|Architecture Canvas]] for the',
    'spatial view. For a specific question, `graphify query "<question>"` is cheaper',
    'than reading any of this.',
    '',
    '## Communities',
    '',
  ]

  const generatedFiles = []
  const fileIndex = new Map()
  let projectedNodes = 0

  for (const community of communities) {
    const related = [...community.related.entries()]
      .filter(([id]) => selected.has(id))
      .sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])))
      .slice(0, RELATED_LIMIT)
    // Every node, densest first — the note is the community's whole contents,
    // not a sample of it.
    const ordered = [...community.nodes]
      .sort((a, b) => (community.degree.get(b.id) ?? 0) - (community.degree.get(a.id) ?? 0)
        || String(a.label).localeCompare(String(b.label)))
    projectedNodes += ordered.length

    const files = new Map()
    for (const node of community.nodes) {
      const file = node.source_file || 'graph'
      files.set(file, (files.get(file) ?? 0) + 1)
      const entry = fileIndex.get(file) ?? new Map()
      entry.set(community.id, (entry.get(community.id) ?? 0) + 1)
      fileIndex.set(file, entry)
    }

    indexLines.push(`- ${noteLink(community)} — ${community.nodes.length} nodes, ${community.internalEdges} internal edges`)
    const note = [
      '---',
      'graphify-generated: true',
      `graphify-community: ${quoted(community.id)}`,
      `graphify-node-count: ${community.nodes.length}`,
      `graphify-internal-edges: ${community.internalEdges}`,
      'tags:',
      '  - graphify/community',
      '---',
      '',
      `# ${community.name}`,
      '',
      `Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].`,
      '',
      '## Source files',
      '',
      ...[...files.entries()]
        .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
        .map(([file, count]) => `- \`${file}\` — ${count} node${count === 1 ? '' : 's'}`),
      '',
      `## Nodes (${ordered.length})`,
      '',
      ...ordered.map(node => `- **${node.label}** — \`${location(node)}\``),
      '',
      '## Related communities',
      '',
      ...(related.length
        ? related.map(([id, weight]) => `- ${noteLink(selected.get(id))} — ${weight} cross-community edges`)
        : ['- No selected community has a direct edge to this one.']),
      '',
    ].join('\n')
    const file = join(outputDir, 'communities', communityFile(community))
    writeFileSync(file, note)
    generatedFiles.push(relative(outputDir, file))
  }

  writeFileSync(join(outputDir, 'Knowledge Map.md'), `${indexLines.join('\n')}\n`)
  generatedFiles.push('Knowledge Map.md')

  // Path -> community. An agent holding a file path can reach the right note
  // without scanning every one of them.
  const fileLines = [
    '---',
    'graphify-generated: true',
    'tags:',
    '  - graphify/files',
    '---',
    '',
    '# Files',
    '',
    '> Generated from `graphify-out/graph.json` by `scripts/graph.sh obsidian`.',
    '',
    `Every indexed source file, and the communities its nodes fall into. Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].`,
    '',
  ]
  for (const [file, entry] of [...fileIndex.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
    const links = [...entry.entries()]
      // Equal counts must not fall back to Map insertion order, or the same
      // graph projects a different file on every run.
      .sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])))
      .map(([id, count]) => `${noteLink(selected.get(id))} (${count})`)
      .join(', ')
    fileLines.push(`- \`${file}\` — ${links}`)
  }
  writeFileSync(join(outputDir, 'Files.md'), `${fileLines.join('\n')}\n`)
  generatedFiles.push('Files.md')

  const canvasCommunities = communities.slice(0, CANVAS_COMMUNITY_LIMIT)
  const onCanvas = new Set(canvasCommunities.map(community => community.id))
  const canvasNodes = canvasCommunities.map((community, index) => ({
    id: `community-${community.id}`,
    type: 'file',
    file: `docs/knowledge/communities/${communityFile(community)}`,
    x: (index % 4) * 420,
    y: Math.floor(index / 4) * 300,
    width: 360,
    height: 220,
  }))
  const canvasEdges = []
  const seen = new Set()
  for (const community of canvasCommunities) {
    for (const [relatedId, weight] of community.related) {
      if (!onCanvas.has(relatedId)) continue
      const pair = [String(community.id), String(relatedId)].sort()
      const key = pair.join(':')
      if (seen.has(key)) continue
      seen.add(key)
      canvasEdges.push({
        id: `edge-${pair[0]}-${pair[1]}`,
        fromNode: `community-${pair[0]}`,
        toNode: `community-${pair[1]}`,
        label: String(weight),
      })
    }
  }
  canvasEdges.sort((a, b) => Number(b.label) - Number(a.label))
  writeFileSync(join(outputDir, 'Architecture.canvas'), JSON.stringify({
    nodes: canvasNodes,
    edges: canvasEdges.slice(0, CANVAS_EDGE_LIMIT),
  }, null, 2))
  generatedFiles.push('Architecture.canvas')

  const manifestPath = join(outputDir, '.generated.json')
  let previous = []
  try { previous = JSON.parse(readFileSync(manifestPath, 'utf8')).files ?? [] } catch { /* first generation */ }
  for (const oldFile of previous) {
    if (!generatedFiles.includes(oldFile)) rmSync(join(outputDir, oldFile), { force: true })
  }
  writeFileSync(manifestPath, `${JSON.stringify({ files: generatedFiles }, null, 2)}\n`)
  return {
    communities: communities.length,
    edges: Math.min(canvasEdges.length, CANVAS_EDGE_LIMIT),
    nodes: projectedNodes,
    files: fileIndex.size,
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const configured = process.env.OBSIDIAN_COMMUNITY_LIMIT
  const result = generateKnowledge({
    graphPath: process.env.GRAPHIFY_GRAPH ?? join(root, 'graphify-out', 'graph.json'),
    outputDir: process.env.OBSIDIAN_KNOWLEDGE_DIR ?? join(root, 'docs', 'knowledge'),
    limit: configured ? Number(configured) : Infinity,
  })
  console.log(
    `Generated ${result.communities} community notes covering ${result.nodes} nodes `
    + `across ${result.files} files, and ${result.edges} Canvas edges.`,
  )
}
