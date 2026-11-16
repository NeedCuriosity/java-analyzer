import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Comparator;
import java.util.EnumMap;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.Function;
import java.util.stream.Stream;

enum Color { RED }

class Gen<T> {
    T value;
    List<T> items = new ArrayList<>();

    Optional<Thread> wrapped() {
        return Optional.ofNullable(/*@tv1*/value);
    }

    List<String> strings() {
        return /*@tv2*/items;
    }

    void put(Map<Thread, T> m, String s) {
        m./*@tv3*/put(s, value);
    }

    String unwrap() {
        return /*@tv4*/value;
    }

    Comparable<T> comparable() {
        for (Comparable<T> c : /*@tv6*/items) {
            return c;
        }
        return /*@tv7*/value;
    }

    T fromList(List<String> xs) {
        return /*@tv5*/xs;
    }

    Object[] arrays(int[] ints, Object[] objs) {
        String[] a = /*@arr1*/ints;
        String[] b = /*@arr2*/objs;
        return a.length > 0 ? b : a;
    }
}

class Main {
    List<String> strings = new ArrayList<>();
    List<Integer> ints = new ArrayList<>();
    Optional<Integer> maybeInt = Optional.empty();

    Optional<String> ret() {
        return /*@ret*/maybeInt;
    }

    Map<String, List<Integer>> nested() {
        Map<String, List<Number>> m = new HashMap<>();
        return /*@retnested*/m;
    }

    void takeStrings(List<String> xs) {
    }

    void locals(List<? extends Number> numbers, Comparator<? super String> cmp, List<?> any) {
        List<String> a = new /*@newint*/ArrayList<Integer>();
        List<Object> b = /*@invariant*/strings;
        List<? extends Number> c = /*@extendsbad*/strings;
        List<Number> d = /*@capture*/numbers;
        Comparator<String> e = /*@supercap*/cmp;
        List<Object> f = /*@anyobj*/any;
        List<? super Integer> g = /*@anysuper*/any;
        Collection<Integer> h = /*@subclass*/strings;
        Map<String, Integer> m = Map.of("a", /*@mapof*/"b");
        List<String> l = /*@aslist*/Arrays.asList(1, 2);
        strings = /*@assign*/ints;
        /*@arg*/takeStrings(ints);
        List<Integer> dia = new ArrayList<>(/*@dia*/strings);
        Map<Thread, String> dia2 = new EnumMap<>(/*@dia2*/Color.class);
        System.out.println(dia.size() + dia2.size());
        System.out.println(a.size() + b.size() + c.size() + d.size() + e.hashCode() + f.size()
                + g.size() + h.size() + m.size() + l.size());
    }

    void loops(Map<String, List<String>> index) {
        for (List<Integer> l : /*@loop1*/index.values()) {
            System.out.println(l);
        }
        for (Map.Entry<Integer, List<String>> e : /*@loop2*/index.entrySet()) {
            System.out.println(e);
        }
    }

    void lambdas(Function<Integer, List<Integer>> wrap) {
        Stream<String> s = ints.stream().map(i -> /*@lam*/i + 1);
        Optional<String> o = maybeInt.map(x -> /*@lam2*/x * 2);
        Stream<Long> t = ints.stream().map(i -> {
            return /*@lam3*/i;
        });
        Stream<List<String>> u = ints.stream().map(i -> /*@lam4*/wrap.apply(i));
        Optional<List<String>> v = maybeInt.map(/*@lam5*/wrap);
        System.out.println(s.count() + t.count() + u.count() + o.hashCode() + v.hashCode());
    }
}
//? diag type-mismatch ret
//? diag type-mismatch retnested
//? diag type-mismatch newint
//? diag type-mismatch invariant
//? diag type-mismatch extendsbad
//? diag type-mismatch capture
//? diag type-mismatch supercap
//? diag type-mismatch anyobj
//? diag type-mismatch anysuper
//? diag type-mismatch subclass
//? diag type-mismatch mapof
//? diag type-mismatch aslist
//? diag type-mismatch assign
//? diag argument-mismatch arg
//? diag type-mismatch lam
//? diag type-mismatch lam2
//? diag type-mismatch lam3
//? diag type-mismatch lam4
//? diag type-mismatch lam5
//? diag type-mismatch tv1
//? diag type-mismatch tv2
//? diag argument-mismatch tv3
//? diag type-mismatch tv4
//? diag type-mismatch tv5
//? diag type-mismatch tv6
//? diag type-mismatch tv7
//? diag type-mismatch arr1
//? diag type-mismatch arr2
//? diag type-mismatch dia
//? diag type-mismatch dia2
//? diag type-mismatch loop1
//? diag type-mismatch loop2
//? diag exact
