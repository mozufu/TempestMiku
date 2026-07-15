package org.mozufu.tempestmiku

/** Keeps only the newest event until Flutter attaches, then delivers it once. */
internal class SinglePendingEventBuffer<T : Any> {
    private var pending: T? = null

    fun offer(value: T, consumer: ((T) -> Unit)?) {
        if (consumer == null) {
            pending = value
        } else {
            consumer(value)
        }
    }

    fun drain(consumer: (T) -> Unit): Boolean {
        val value = pending ?: return false
        pending = null
        consumer(value)
        return true
    }
}
