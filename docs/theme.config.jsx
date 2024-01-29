import { useRouter } from 'next/router'
import { useConfig } from 'nextra-theme-docs'
import Logo from './components/logo'

export default {
    logo: <div style={{ height: 25 }}><Logo /></div>,
    project: {
        link: 'https://github.com/drifting-in-space/plane'
    },
    chat: {
        link: "https://discord.gg/N5sEpsuhh9"
    },
    footer: {
        text: <span>
            Plane is an open-source project of <a href="https://driftingin.space">Drifting in Space Corp.</a>
        </span>,
    },
    head: () => {
        const { frontMatter, title } = useConfig()
        const { asPath } = useRouter()

        let ogImageUrl = asPath === '/' ? 'https://plane.dev/api/og-image' : `https://plane.dev/api/og-image?title=${encodeURIComponent(title)}`

        return <>
            <meta
                property="og:description"
                content={frontMatter.description || 'An open-source distributed system for running WebSocket backends at scale'}
            />
            <meta property="og:image" content={ogImageUrl} />
            <meta name="twitter:image" content={ogImageUrl} />
            <meta name="twitter:card" content="summary_large_image" />
            <meta name="twitter:site" content="@drifting_corp" />
            <meta name="twitter:title" content="Plane" />
            <meta name="twitter:description" content="An open-source distributed system for scaling WebSocket backends." />
        </>
    },
    useNextSeoProps() {
        const { asPath } = useRouter()
        if (asPath !== '/') {
            return {
                titleTemplate: '%s – Plane'
            }
        } else {
            return {
                title: 'Plane – run WebSocket backends at scale'
            }
        }
    },
    nextThemes: {
        defaultTheme: 'dark',
    },
    docsRepositoryBase: 'https://github.com/drifting-in-space/plane/tree/main/docs',
}
