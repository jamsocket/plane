import React from 'react'
import styles from './button.module.css'

export default function Button(props) {
    return <a href={props.url} className={styles.button + ' ' + (props.primary ? styles.primary : '')}>
        <span>{props.text}</span>
    </a>
}
