import java.io.Serializable;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

class Main<E> {
    static class RawList extends ArrayList {
        void fill() {
            /*@implicit_call*/add("x");
        }
    }

    static class Holder<T> {
        void put(List<T> more) {}
    }

    static List rawFactory() {
        return new ArrayList();
    }

    List<String> conversions(List raw, Collection rawColl) {
        List<String> a = /*@local*/raw;
        List<String> b = /*@new_raw*/new ArrayList();
        List<?> ok1 = raw;
        List ok2 = raw;
        List<String> c = /*@factory*/rawFactory();
        List<String> d = /*@subclass*/new RawList();
        Collection<String> e = /*@coll*/rawColl;
        a = /*@assign*/raw;
        System.out.println(a.size() + b.size() + c.size() + d.size() + e.size() + ok1.size() + ok2.size());
        return /*@ret*/raw;
    }

    void calls(List raw, Map rawMap, Holder<String> holder) {
        raw./*@call_add*/add("x");
        raw.size();
        raw.contains("z");
        rawMap./*@call_put*/put("k", "v");
        rawMap.get("k");
        holder./*@invoke*/put(/*@invoke_arg*/raw);
        Collections./*@sort*/sort(/*@sort_arg*/raw);
        List copy = new /*@raw_ctor*/ArrayList(raw);
        List sized = new ArrayList(10);
        Map copyMap = new /*@raw_map_ctor*/HashMap(rawMap);
        System.out.println(copy.size() + sized.size() + copyMap.size());
    }

    <T> T casts(Object object, List<String> strings, Collection<String> collection, Serializable serializable, E element) {
        List<String> a = (/*@cast_list*/List<String>) object;
        List<?> ok1 = (List<?>) object;
        List ok2 = (List) object;
        ArrayList<String> ok3 = (ArrayList<String>) strings;
        List<String> ok4 = (List<String>) collection;
        T f = (/*@cast_t*/T) object;
        E ok5 = (E) element;
        ArrayList<String> g = (/*@cast_side*/ArrayList<String>) serializable;
        T[] h = (/*@cast_array*/T[]) new Object[3];
        Object[] ok6 = (Object[]) new String[1];
        System.out.println(a + "" + ok1 + ok2 + ok3 + ok4 + ok5 + g + h.length + ok6.length);
        return f;
    }

    @SuppressWarnings("unchecked")
    void suppressed(List raw) {
        raw.add("x");
        List<String> typed = raw;
        System.out.println(typed);
    }

    void byComment(List raw) {
        //noinspection unchecked
        raw.add("x");
        raw./*@fix_call*/add("y");
    }
}
//? diag unchecked implicit_call
//? diag unchecked local
//? diag unchecked new_raw
//? diag unchecked factory
//? diag unchecked subclass
//? diag unchecked coll
//? diag unchecked assign
//? diag unchecked ret
//? diag unchecked call_add
//? diag unchecked call_put
//? diag unchecked invoke
//? diag unchecked invoke_arg
//? diag unchecked sort
//? diag unchecked sort_arg
//? diag unchecked raw_ctor
//? diag unchecked raw_map_ctor
//? diag unchecked cast_list
//? diag unchecked cast_t
//? diag unchecked cast_side
//? diag unchecked cast_array
//? diag unchecked fix_call
//? diag only unchecked
//? diag none
//? assist suppress_method fix_call ; has "@SuppressWarnings(\"unchecked\")\n    void byComment(List raw) {"
//? assist suppress_statement fix_call ; has "        //noinspection unchecked\n        raw.add(\"y\");"
