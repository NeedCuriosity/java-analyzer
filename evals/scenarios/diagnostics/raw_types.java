import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;

class Main implements /*@impl*/Comparable {
    /*@field*/List names;
    Map.Entry<String, Integer> entry;
    Map./*@entry*/Entry rawEntry;

    public int compareTo(Object o) {
        return 0;
    }

    void run(Object o) {
        List<String> xs = new /*@newr*/ArrayList();
        /*@local*/List ys = new ArrayList<>();
        List<String> zs = new ArrayList<>();
        // 语言要求或者没法写类型实参的地方
        boolean isList = o instanceof List;
        /*@castdecl*/List cast = (List) o;
        /*@arrdecl*/List[] arrays = new List[3];
        Class<?> k = List.class;
        Comparator<String> c = String::compareTo;
        System.out.println(xs.size() + ys.size() + zs.size() + arrays.length + (isList ? 1 : 0) + cast.size() + k.getName() + c);
    }
}

class Child extends ArrayList<String> {
    @Override
    public boolean removeAll(java.util.Collection c) {
        return super.removeAll(c);
    }
}

//? diag raw-type impl
//? diag raw-type field
//? diag raw-type entry
//? diag raw-type newr
//? diag raw-type local
//? diag raw-type castdecl
//? diag raw-type arrdecl
//? diag only raw-type
//? assist fix_redundancy newr ; has "List<String> xs = new ArrayList<>();"
