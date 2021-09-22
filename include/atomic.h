#ifndef ATOMIC_H
#define ATOMIC_H

#ifdef __cplusplus
#include <atomic>
#endif

#ifdef __cplusplus
#define atomic64_t std::atomic<uint64_t>
#else
#define atomic64_t _Atomic uint64_t
#endif

#ifdef __cplusplus
#define atomic32_t std::atomic<uint32_t>
#else
#define atomic32_t _Atomic uint32_t
#endif

#ifdef __cplusplus
template <typename T>
/**
 * @brief compatible function for stdbool.h
 *
 */
static inline void atomic_store(std::atomic<T> *v, int arg)
{
	*v = arg;
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T> static inline T atomic_load(std::atomic<T> *v)
{
	return *v;
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T>
static inline void atomic_fetch_add(std::atomic<T> *v, int arg)
{
	std::atomic_fetch_add(v, static_cast<T>(arg));
}

/**
 * @brief compatible function for stdbool.h
 *
 */
template <class T>
static inline void atomic_fetch_sub(std::atomic<T> *v, int arg)
{
	std::atomic_fetch_sub(v, static_cast<T>(arg));
}
#endif

#endif
