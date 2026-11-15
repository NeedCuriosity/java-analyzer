import java.util.ArrayList;
import java.util.List;
import java.util.Map;

class Main {
    int run(/*@list_ty*/List<String> names, Map<String, List<Integer>> index) {
        names./*@add*/add("x");
        /*@string_ty*/String first = names./*@get*/get(0);
        System./*@out*/out./*@println*/println(first./*@length*/length());
        List<Integer> hits = index./*@map_get*/get(first);
        return hits./*@size*/size() + new /*@array_list*/ArrayList<String>(names).size();
    }
}
//? def list_ty -> lib java.util.List
//? def add -> lib java.util.List#add
//? def string_ty -> lib java.lang.String
//? def get -> lib java.util.List#get
//? def out -> lib java.lang.System#out
//? def println -> lib java.io.PrintStream#println
//? def length -> lib java.lang.String#length
//? def map_get -> lib java.util.Map#get
//? def size -> lib java.util.List#size
//? typedef first_use -> lib java.lang.String
//? diag none
class Other {
    int f(String /*@first_decl*/first) { return /*@first_use*/first.length(); }
}
