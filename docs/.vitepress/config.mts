import {withMermaid} from 'vitepress-plugin-mermaid';
import {readFileSync} from 'fs';
import {resolve} from 'path';

const cargoToml = readFileSync(
    resolve(__dirname, '../../Cargo.toml'),
    'utf8'
);
const version = cargoToml.match(/version = "(.*?)"/)[1];
if (!version) {
  throw new Error('Version not found in Cargo.toml');
}

const base = '/firepit/';

// https://vitepress.dev/reference/site-config
export default withMermaid({
  title: "Firepit",
  description: "🏕 A simple, powerful task runner with service management and a comfy terminal UI",
  base,
  head: [['link', { rel: 'icon', href: `${base}favicon.ico` }]],
  themeConfig: {
    logo: 'logo.png',
    nav: [
      { text: 'Home', link: '/' },
      { text: 'Docs', link: '/configuration' },
      { text: `v${version}`,
        items: [
          {
            text: `v${version}`,
            link: `https://github.com/kota65535/firepit/releases/tag/${version}`
          },
          {
            text: 'ChangeLog',
            link: 'https://github.com/kota65535/firepit/blob/main/CHANGELOG.md'
          },
        ]
      }
    ],

    sidebar: [
      {
        text: 'Installation',
        link: '/installation'
      },
      {
        text: 'Getting Started',
        link: '/getting-started'
      },
      {
        text: 'Configuration',
        link: '/configuration'
      },
      {
        text: 'TUI',
        link: '/tui'
      },
      {
        text: 'CLI',
        link: '/cli'
      },
      {
        text: 'Schema',
        link: '/schema'
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/kota65535/firepit' }
    ]
  },
  mermaid: { theme: 'forest' },
})
