package org.mozufu.tempestmiku

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SinglePendingEventBufferTest {
    @Test
    fun keepsOnlyNewestColdStartEventAndDrainsItOnce() {
        val buffer = SinglePendingEventBuffer<String>()
        val delivered = mutableListOf<String>()

        buffer.offer("first", consumer = null)
        buffer.offer("newest", consumer = null)

        assertTrue(buffer.drain(delivered::add))
        assertEquals(listOf("newest"), delivered)
        assertFalse(buffer.drain(delivered::add))
    }

    @Test
    fun sendsWarmEventsDirectlyWithoutRetainingThem() {
        val buffer = SinglePendingEventBuffer<String>()
        val delivered = mutableListOf<String>()

        buffer.offer("warm", delivered::add)

        assertEquals(listOf("warm"), delivered)
        assertFalse(buffer.drain(delivered::add))
    }
}
