export type DesktopStorageKey = 'channels'

const DB_NAME = 'heigate-desktop'
const DB_VERSION = 1
const STORE_NAME = 'workspace'
const FALLBACK_PREFIX = 'heigate.desktop.'
const LEGACY_PREFIX = 'sub2api.desktop.'

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (typeof indexedDB === 'undefined') {
      reject(new Error('indexeddb-unavailable'))
      return
    }
    const request = indexedDB.open(DB_NAME, DB_VERSION)
    request.onerror = () => reject(request.error ?? new Error('indexeddb-open-failed'))
    request.onsuccess = () => resolve(request.result)
    request.onupgradeneeded = () => {
      const database = request.result
      if (!database.objectStoreNames.contains(STORE_NAME)) database.createObjectStore(STORE_NAME)
    }
  })
}

async function withStore<T>(mode: IDBTransactionMode, operation: (store: IDBObjectStore) => IDBRequest): Promise<T> {
  const database = await openDatabase()
  return new Promise((resolve, reject) => {
    const transaction = database.transaction(STORE_NAME, mode)
    const request = operation(transaction.objectStore(STORE_NAME))
    request.onerror = () => reject(request.error ?? new Error('indexeddb-request-failed'))
    request.onsuccess = () => resolve(request.result as T)
    transaction.oncomplete = () => database.close()
    transaction.onerror = () => reject(transaction.error ?? new Error('indexeddb-transaction-failed'))
  })
}

export async function readDesktopValue<T>(key: DesktopStorageKey): Promise<T | null> {
  try {
    const value = await withStore<T | undefined>('readonly', (store) => store.get(key))
    return value ?? null
  } catch {
    const raw = localStorage.getItem(`${FALLBACK_PREFIX}${key}`) ?? localStorage.getItem(`${LEGACY_PREFIX}${key}`)
    if (!raw) return null
    try {
      return JSON.parse(raw) as T
    } catch {
      return null
    }
  }
}

export async function writeDesktopValue<T>(key: DesktopStorageKey, value: T): Promise<void> {
  try {
    await withStore('readwrite', (store) => store.put(value, key))
  } catch {
    localStorage.setItem(`${FALLBACK_PREFIX}${key}`, JSON.stringify(value))
  }
}

export async function clearDesktopValue(key: DesktopStorageKey): Promise<void> {
  try {
    await withStore('readwrite', (store) => store.delete(key))
  } catch {
    localStorage.removeItem(`${FALLBACK_PREFIX}${key}`)
  }
}
