//- /org/check/Assertions.java
package org.check;

import java.util.Collection;
import java.util.List;

// 和 AssertJ 一样的重载：`<T> assertThat(T)` 能接受任何实参，但声明的形参是类型变量，
// 比 `assertThat(List<? extends E>)` 这些泛型重载都更不具体
public class Assertions {
    public static <T> ObjectAssert<T> assertThat(T actual) { return new ObjectAssert<>(); }
    public static <T extends Comparable<? super T>> ComparableAssert<T> assertThat(T actual) { return new ComparableAssert<>(); }
    public static <E> IterableAssert<E> assertThat(Iterable<? extends E> actual) { return new IterableAssert<>(); }
    public static <E> CollectionAssert<E> assertThat(Collection<? extends E> actual) { return new CollectionAssert<>(); }
    public static <E> ListAssert<E> assertThat(List<? extends E> actual) { return new ListAssert<>(); }
    public static StringAssert assertThat(String actual) { return new StringAssert(); }
}

//- /org/check/ObjectAssert.java
package org.check;
public class ObjectAssert<T> { public ObjectAssert<T> isNotNull() { return this; } }

//- /org/check/ComparableAssert.java
package org.check;
public class ComparableAssert<T> { public ComparableAssert<T> isLessThan(T other) { return this; } }

//- /org/check/IterableAssert.java
package org.check;
public class IterableAssert<E> { public IterableAssert<E> isEmpty() { return this; } }

//- /org/check/CollectionAssert.java
package org.check;
public class CollectionAssert<E> { public CollectionAssert<E> hasSize(int n) { return this; } }

//- /org/check/ListAssert.java
package org.check;
public class ListAssert<E> { public ListAssert<E> hasSize(int n) { return this; } public ListAssert<E> first() { return this; } }

//- /org/check/StringAssert.java
package org.check;
public class StringAssert { public StringAssert startsWith(String p) { return this; } }

//- /app/Pages.java
package app;

import java.util.Iterator;
import java.util.List;

public class Pages<T> implements Iterable<T> {
    public Iterator<T> iterator() { return List.<T>of().iterator(); }
}

//- /app/CheckTest.java
package app;

import static org.check.Assertions.assertThat;

import java.util.List;
import java.util.Set;

class CheckTest {
    void run(List<String> names, Set<Integer> ids, Pages<String> page, Object any, Integer n) {
        var list = assertThat(names);
        list.hasSize(2).first();
        var set = assertThat(ids);
        set.hasSize(1);
        var iterable = assertThat(page);
        iterable.isEmpty();
        var object = assertThat(any);
        object.isNotNull();
        var comparable = assertThat(n);
        comparable.isLessThan(3);
        assertThat("x").startsWith("y");
        /*@listVar*/list.hasSize(1);
        /*@iterVar*/iterable.isEmpty();
        /*@objVar*/object.isNotNull();
        /*@cmpVar*/comparable.isLessThan(1);
    }
}
//? hover listVar ~ "ListAssert<String>"
//? hover iterVar ~ "IterableAssert<String>"
//? hover objVar ~ "ObjectAssert<Object>"
//? hover cmpVar ~ "ComparableAssert<Integer>"
//? diag none
