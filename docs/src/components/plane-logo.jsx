import React from 'react'
import ThemedImage from '@theme/ThemedImage';

const PlaneLogo = () => {
    return <ThemedImage
        alt="Docusaurus themed image"
        sources={{
            light: "img/plane-logo-light.svg",
            dark: "img/plane-logo-dark.svg",
        }}
    />
}

export default PlaneLogo;
