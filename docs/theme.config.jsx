import { useRouter } from 'next/router'
import Logo from './components/logo'

export default {
    logo: <div style={{ height: 25 }}><Logo /></div>,
    project: {
        link: 'https://github.com/drifting-in-space/plane'
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
    }
}
