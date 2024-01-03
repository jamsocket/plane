import { ImageResponse } from 'next/og'
import PlaneLogo from '../../components/logo'
import { NextRequest } from 'next/server';

export const config = {
    runtime: 'edge',
}

export default async function handler(request: NextRequest) {
    const { searchParams } = new URL(request.url);

    const fontData = await fetch(
        new URL('OpenSans-SemiBold.ttf', import.meta.url),
    ).then((res) => res.arrayBuffer());

    const title = searchParams.get('title')

    return new ImageResponse(
        (
            <div
                style={{
                    display: 'flex',
                    flexDirection: 'column',
                    justifyContent: 'center',
                    alignItems: 'center',
                    width: '100%',
                    height: '100%',
                    background: '#011029',
                    color: 'white',
                    fontFamily: 'Open Sans',
                }}
            >
                <PlaneLogo height={180} />
                {title && (
                <div style={{ marginTop: 50, fontSize: 60, display: 'flex' }}>
                    <span style={{color: '#555'}}>docs/</span>{title}
                </div>
                )}
            </div>
        ),
        {
            fonts: [
                {
                    name: 'Open Sans',
                    data: fontData,
                    style: 'normal',
                }
            ]
        }
    )
}
