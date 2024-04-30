use secular::lower_lay_string;
use regex::Regex;
use collapse::collapse;

pub fn prepare_input(input: &str) -> String {
  let mut prepared_input = lower_lay_string(&input);

  let re = Regex::new(r#"[`~!@#$%^&*()_|+\-=?;:",.<>\{\}\[\]\\\/]"#).unwrap();
  prepared_input = re.replace_all(&prepared_input, " ").to_string();

  let re = Regex::new(r#"['’]"#).unwrap();
  prepared_input = re.replace_all(&prepared_input, "").to_string();

  prepared_input = prepared_input.to_lowercase();
  prepared_input = collapse(&prepared_input);

  prepared_input
}
