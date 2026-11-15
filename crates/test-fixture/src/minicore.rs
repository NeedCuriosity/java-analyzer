//! 标准库的最小源码桩：只有测试用得到的类型和成员，方法体全部省略为最简形式。

pub const MINICORE: &str = r#"
//- /minicore/java/lang/Object.java
package java.lang;
public class Object {
    public Object() {}
    public boolean equals(Object obj) { return this == obj; }
    public int hashCode() { return 0; }
    public String toString() { return null; }
    public final Class<?> getClass() { return null; }
}
//- /minicore/java/lang/Class.java
package java.lang;
public final class Class<T> {
    public String getName() { return null; }
    public String getSimpleName() { return null; }
}
//- /minicore/java/lang/CharSequence.java
package java.lang;
public interface CharSequence {
    int length();
    char charAt(int index);
}
//- /minicore/java/lang/Comparable.java
package java.lang;
public interface Comparable<T> {
    int compareTo(T o);
}
//- /minicore/java/lang/String.java
package java.lang;
public final class String implements CharSequence, Comparable<String> {
    public String() {}
    public int length() { return 0; }
    public char charAt(int index) { return 0; }
    public int compareTo(String other) { return 0; }
    public boolean isEmpty() { return true; }
    public String substring(int beginIndex) { return this; }
    public String substring(int beginIndex, int endIndex) { return this; }
    public String trim() { return this; }
    public String toUpperCase() { return this; }
    public boolean startsWith(String prefix) { return false; }
    public static String valueOf(Object obj) { return null; }
    public static String valueOf(int i) { return null; }
    public static String format(String format, Object... args) { return null; }
}
//- /minicore/java/lang/Number.java
package java.lang;
public abstract class Number {
    public abstract int intValue();
    public abstract long longValue();
    public abstract double doubleValue();
}
//- /minicore/java/lang/Integer.java
package java.lang;
public final class Integer extends Number implements Comparable<Integer> {
    public static final int MAX_VALUE = 2147483647;
    public static final int MIN_VALUE = -2147483648;
    public int intValue() { return 0; }
    public long longValue() { return 0; }
    public double doubleValue() { return 0; }
    public int compareTo(Integer other) { return 0; }
    public static Integer valueOf(int i) { return null; }
    public static int parseInt(String s) { return 0; }
}
//- /minicore/java/lang/Long.java
package java.lang;
public final class Long extends Number {
    public int intValue() { return 0; }
    public long longValue() { return 0; }
    public double doubleValue() { return 0; }
}
//- /minicore/java/lang/Double.java
package java.lang;
public final class Double extends Number {
    public int intValue() { return 0; }
    public long longValue() { return 0; }
    public double doubleValue() { return 0; }
}
//- /minicore/java/lang/Boolean.java
package java.lang;
public final class Boolean {
    public static final Boolean TRUE = null;
    public static final Boolean FALSE = null;
    public boolean booleanValue() { return false; }
}
//- /minicore/java/lang/Character.java
package java.lang;
public final class Character {
    public char charValue() { return 0; }
}
//- /minicore/java/lang/Void.java
package java.lang;
public final class Void {}
//- /minicore/java/lang/Enum.java
package java.lang;
public abstract class Enum<E extends Enum<E>> implements Comparable<E> {
    public final String name() { return null; }
    public final int ordinal() { return 0; }
    public final int compareTo(E o) { return 0; }
}
//- /minicore/java/lang/Record.java
package java.lang;
public abstract class Record {}
//- /minicore/java/lang/Iterable.java
package java.lang;
public interface Iterable<T> {
    java.util.Iterator<T> iterator();
}
//- /minicore/java/lang/Runnable.java
package java.lang;
@FunctionalInterface
public interface Runnable {
    void run();
}
//- /minicore/java/lang/AutoCloseable.java
package java.lang;
public interface AutoCloseable {
    void close() throws Exception;
}
//- /minicore/java/lang/Throwable.java
package java.lang;
public class Throwable {
    public Throwable() {}
    public Throwable(String message) {}
    public String getMessage() { return null; }
    public void printStackTrace() {}
}
//- /minicore/java/lang/Exception.java
package java.lang;
public class Exception extends Throwable {
    public Exception() {}
    public Exception(String message) {}
}
//- /minicore/java/lang/RuntimeException.java
package java.lang;
public class RuntimeException extends Exception {
    public RuntimeException() {}
    public RuntimeException(String message) {}
}
//- /minicore/java/lang/IllegalStateException.java
package java.lang;
public class IllegalStateException extends RuntimeException {
    public IllegalStateException() {}
    public IllegalStateException(String message) {}
}
//- /minicore/java/lang/UnsupportedOperationException.java
package java.lang;
public class UnsupportedOperationException extends RuntimeException {
    public UnsupportedOperationException() {}
}
//- /minicore/java/lang/Override.java
package java.lang;
public @interface Override {}
//- /minicore/java/lang/Deprecated.java
package java.lang;
public @interface Deprecated {
    String since() default "";
    boolean forRemoval() default false;
}
//- /minicore/java/lang/FunctionalInterface.java
package java.lang;
public @interface FunctionalInterface {}
//- /minicore/java/lang/SuppressWarnings.java
package java.lang;
public @interface SuppressWarnings {
    String[] value();
}
//- /minicore/java/lang/System.java
package java.lang;
public final class System {
    public static final java.io.PrintStream out = null;
    public static final java.io.PrintStream err = null;
    public static long currentTimeMillis() { return 0; }
}
//- /minicore/java/lang/Math.java
package java.lang;
public final class Math {
    public static final double PI = 3.14;
    public static int max(int a, int b) { return a; }
    public static int min(int a, int b) { return a; }
    public static int abs(int a) { return a; }
}
//- /minicore/java/lang/StringBuilder.java
package java.lang;
public final class StringBuilder implements CharSequence {
    public StringBuilder() {}
    public StringBuilder append(String s) { return this; }
    public StringBuilder append(int i) { return this; }
    public StringBuilder append(Object o) { return this; }
    public int length() { return 0; }
    public char charAt(int index) { return 0; }
}
//- /minicore/java/io/PrintStream.java
package java.io;
public class PrintStream {
    public void println() {}
    public void println(String x) {}
    public void println(int x) {}
    public void println(Object x) {}
    public void print(String s) {}
}
//- /minicore/java/io/IOException.java
package java.io;
public class IOException extends Exception {
    public IOException() {}
    public IOException(String message) {}
}
//- /minicore/java/util/Iterator.java
package java.util;
public interface Iterator<E> {
    boolean hasNext();
    E next();
}
//- /minicore/java/util/Collection.java
package java.util;
public interface Collection<E> extends Iterable<E> {
    int size();
    boolean isEmpty();
    boolean contains(Object o);
    boolean add(E e);
    java.util.stream.Stream<E> stream();
}
//- /minicore/java/util/List.java
package java.util;
public interface List<E> extends Collection<E> {
    E get(int index);
    E set(int index, E element);
    static <T> List<T> of(T... elements) { return null; }
}
//- /minicore/java/util/ArrayList.java
package java.util;
public class ArrayList<E> implements List<E> {
    public ArrayList() {}
    public ArrayList(int initialCapacity) {}
    public int size() { return 0; }
    public boolean isEmpty() { return true; }
    public boolean contains(Object o) { return false; }
    public boolean add(E e) { return true; }
    public E get(int index) { return null; }
    public E set(int index, E element) { return null; }
    public Iterator<E> iterator() { return null; }
    public java.util.stream.Stream<E> stream() { return null; }
}
//- /minicore/java/util/Set.java
package java.util;
public interface Set<E> extends Collection<E> {}
//- /minicore/java/util/Map.java
package java.util;
public interface Map<K, V> {
    V get(Object key);
    V put(K key, V value);
    boolean containsKey(Object key);
    Set<K> keySet();
    Set<Entry<K, V>> entrySet();
    interface Entry<K, V> {
        K getKey();
        V getValue();
    }
}
//- /minicore/java/util/HashMap.java
package java.util;
public class HashMap<K, V> implements Map<K, V> {
    public HashMap() {}
    public V get(Object key) { return null; }
    public V put(K key, V value) { return null; }
    public boolean containsKey(Object key) { return false; }
    public Set<K> keySet() { return null; }
    public Set<Map.Entry<K, V>> entrySet() { return null; }
}
//- /minicore/java/util/Optional.java
package java.util;
public final class Optional<T> {
    public static <T> Optional<T> of(T value) { return null; }
    public static <T> Optional<T> empty() { return null; }
    public T get() { return null; }
    public boolean isPresent() { return false; }
    public T orElse(T other) { return other; }
    public <U> Optional<U> map(java.util.function.Function<? super T, ? extends U> mapper) { return null; }
}
//- /minicore/java/util/function/Function.java
package java.util.function;
@FunctionalInterface
public interface Function<T, R> {
    R apply(T t);
}
//- /minicore/java/util/function/Supplier.java
package java.util.function;
@FunctionalInterface
public interface Supplier<T> {
    T get();
}
//- /minicore/java/util/function/Consumer.java
package java.util.function;
@FunctionalInterface
public interface Consumer<T> {
    void accept(T t);
}
//- /minicore/java/util/function/Predicate.java
package java.util.function;
@FunctionalInterface
public interface Predicate<T> {
    boolean test(T t);
}
//- /minicore/java/util/stream/Stream.java
package java.util.stream;
public interface Stream<T> {
    Stream<T> filter(java.util.function.Predicate<? super T> predicate);
    <R> Stream<R> map(java.util.function.Function<? super T, ? extends R> mapper);
    void forEach(java.util.function.Consumer<? super T> action);
    java.util.List<T> toList();
    long count();
}
"#;
