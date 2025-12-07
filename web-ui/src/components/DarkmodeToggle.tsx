import styles from './DarkmodeToggle.module.css' // Import the CSS module

interface Props {
  onThemeToggle: () => void
  isDarkMode: boolean
}

const DarkmodeToggle = ({ onThemeToggle, isDarkMode }: Props) => {
  return (
    <label className={styles.themeToggle}>
      <input
        type="checkbox"
        className={styles.themeToggleInput}
        checked={isDarkMode}
        onChange={onThemeToggle}
      />
      <div className={styles.themeToggleTrack}>
        <div className={styles.themeToggleThumb}>
          {isDarkMode ? (
            <span
              className={`${styles.themeToggleIcon} ${styles.themeToggleIconMoon}`}
            >
              🌚
            </span> // Moon face icon
          ) : (
            <span
              className={`${styles.themeToggleIcon} ${styles.themeToggleIconSun}`}
            >
              🌞
            </span> // Sun icon
          )}
        </div>
      </div>
    </label>
  )
}

export default DarkmodeToggle
