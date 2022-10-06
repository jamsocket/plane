// @ts-check
// Note: type annotations allow type checking and IDEs autocompletion

const lightCodeTheme = require('prism-react-renderer/themes/github');
const darkCodeTheme = require('prism-react-renderer/themes/dracula');

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'Plane Docs',
  tagline: 'Session backend orchestrator for ambitious browser-based apps.',
  url: 'https://plane.dev',
  baseUrl: '/',
  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',
  favicon: 'img/favicon.png',

  // GitHub pages deployment config.
  // If you aren't using GitHub pages, you don't need these.
  organizationName: 'drifting-in-space', // Usually your GitHub org/user name.
  projectName: 'plane', // Usually your repo name.

  // Even if you don't use internalization, you can use this field to set useful
  // metadata like html lang. For example, if your site is Chinese, you may want
  // to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          sidebarPath: require.resolve('./sidebars.js'),
          // Please change this to your repo.
          // Remove this to remove the "edit this page" links.
          // editUrl:
          //   'https://github.com/facebook/docusaurus/tree/main/packages/create-docusaurus/templates/shared/',
        },
        theme: {
          customCss: require.resolve('./src/css/custom.css'),
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: "/img/share-card.png",
      colorMode: {
        defaultMode: 'dark',
      },
      navbar: {
        logo: {
          alt: 'Plane Logo',
          src: 'img/plane-logo-light.svg',
          srcDark: 'img/plane-logo-dark.svg'
        },
        // items: [
        //   {
        //     type: 'doc',
        //     docId: 'intro',
        //     position: 'left',
        //     label: 'Tutorial',
        //   },
        //   {
        //     href: 'https://github.com/drifting-in-space/plane',
        //     label: 'GitHub',
        //     position: 'right',
        //   },
        // ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {
                label: 'Getting Started',
                to: '/docs/getting-started',
              },
              {
                label: 'Concepts',
                to: '/docs/concepts',
              },
              {
                label: 'Deploying',
                to: '/docs/deploying',
              },
            ],
          },
          {
            title: 'Links',
            items: [
              {
                label: 'Discord',
                href: 'https://discord.gg/N5sEpsuhh9',
              },
              {
                label: 'Twitter',
                href: 'https://twitter.com/drifting_corp',
              },
              {
                label: 'GitHub',
                href: 'https://github.com/drifting-in-space/plane',
              },
              {
                label: 'Drifting in Space',
                href: 'https://driftingin.space/'
              },
            ],
          },
        ],
        copyright: `Copyright © ${new Date().getFullYear()} Drifting in Space, Corp.`,
      },
      prism: {
        theme: lightCodeTheme,
        darkTheme: darkCodeTheme,
      },
    }),
  scripts: [
    { src: 'https://plausible.io/js/plausible.js', defer: true, 'data-domain': 'plane.dev' }
  ]
};

module.exports = config;
