import type { AppProps } from 'next/app'
import Head from 'next/head'
import Script from 'next/script'

export default function App({ Component, pageProps }: AppProps) {
  return (
    <>
      <Head>
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <link rel="icon" href="/favicon.png" />
        <meta property="og:image" content="https://y-sweet.cloud/og-image.png" />
        <meta name="twitter:image" content="https://y-sweet.cloud/og-image.png" />
        <meta name="twitter:card" content="summary_large_image" />
        <meta name="twitter:site" content="@drifting_corp" />
        <meta name="twitter:title" content="Y-Sweet Cloud" />
        <meta name="twitter:description" content="a Yjs service with persistence and auth" />
        <title>Y-Sweet Cloud: the WebSocket sync backend you’ll love</title>
      </Head>
      <Component {...pageProps} />
      <Script defer data-domain="y-sweet.cloud" src="/js/script.js"></Script>
    </>
  )
}
