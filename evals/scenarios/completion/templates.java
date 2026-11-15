import java.util.List;
import java.util.Map;

class Main {
    void print(String label, int count) {
        int total = count * 2;
        soutv/*@soutv*/
    }

    void params(String label, int count) {
        soutp/*@soutp*/
    }

    void method() {
        soutm/*@soutm*/
    }

    void iterate(List<String> names) {
        iter/*@iter*/
    }

    void entries(Map<String, Integer> scores) {
        var pairs = scores.entrySet();
        iter/*@iter_entries*/
    }

    void arrays(int[] values) {
        itar/*@itar*/
    }

    void reverse(String[] words) {
        ritar/*@ritar*/
    }

    void fail() {
        thr/*@thr*/
    }

    psfs/*@psfs*/
}
//? insert soutv soutv ~ "System.out.println(\"total = \" + total);"
//? insert soutp soutp ~ "System.out.println(\"label = \" + label + \", \" + \"count = \" + count);"
//? insert soutm soutm ~ "System.out.println(\"Main.method\");"
//? insert iter iter ~ "for (${1:String} ${2:name} : names) {"
//? insert iter_entries iter ~ "for (${1:Map.Entry<String, Integer>} ${2:pair} : pairs) {"
//? insert itar itar ~ "int ${2:value} = values[$1];"
//? insert ritar ritar ~ "for (int ${1:i} = words.length - 1; $1 >= 0; $1--) {"
//? insert thr thr ~ "throw new $0"
//? insert psfs psfs ~ "public static final String ${1:NAME} = \"$2\";"
