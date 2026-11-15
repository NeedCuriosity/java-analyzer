import java.util.List;

class Main {
    String join(List<String> items, int count) {
        String result = "";
        for (String item : items) {
            /*@plus_eq*/result += item + ", ";
            result = result + count + 1;
        }
        if (result.length() > 2) {
            System.out.println(result);
        }
        return result;
    }

    String nested(List<String> items) {
        String text = "start";
        for (String item : items) {
            /*@used*/text += text.length();
        }
        return text;
    }
}
//? assist concat_loop_to_builder plus_eq ; has "StringBuilder result = new StringBuilder();" ; has "result.append(item).append(\", \");" ; has "result.append(count).append(1);" ; has "if (result.length() > 2) {" ; has "System.out.println(result.toString());" ; has "return result.toString();"
//? noassist concat_loop_to_builder used
