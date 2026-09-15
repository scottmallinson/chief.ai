/**
 * Conventional Commits, scoped to the areas of this codebase.
 * @see https://www.conventionalcommits.org/
 */
export default {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'scope-enum': [
      2,
      'always',
      [
        'agent', // llama.cpp client, prompt + tool-calling orchestration
        'auth', // Local PKCE OAuth flows
        'corpus', // The markdown corpus, its index, recipes and drafts
        'db', // SQLite schema, migrations, queries
        'daemon', // Background work-log daemon
        'integrations', // GitHub, Calendar and other data sources
        'ui', // React components, styling, layout
        'tauri', // Tauri shell, capabilities, plugins, config
        'deps', // Dependency bumps
        'ci', // Workflows and automation
        'repo', // Tooling, config, docs, meta
        'website', // The marketing site — deployed on its own, and never in a release
      ],
    ],
    'scope-empty': [1, 'never'],
    'body-max-line-length': [0, 'always'],
    'footer-max-line-length': [0, 'always'],
  },
};
