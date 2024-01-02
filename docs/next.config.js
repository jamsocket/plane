const withNextra = require('nextra')({
    theme: 'nextra-theme-docs',
    themeConfig: './theme.config.jsx'
})

module.exports = withNextra({
    async rewrites() {
        return [
            {
                source: '/js/script.js',
                destination: 'https://plausible.io/js/script.js'
            },
            {
                source: '/api/event',
                destination: 'https://plausible.io/api/event'
            }
        ];
    },
})
